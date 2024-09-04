use {
    crate::wiggle_abi::types::{FastlyStatus, KvError, KvInsertMode},
    std::{
        collections::BTreeMap,
        sync::{Arc, RwLock},
    },
};

#[derive(Debug, Clone)]
pub struct ObjectValue {
    pub body: Vec<u8>,
    // these two replace an Option<String> so we can
    // derive Copy
    pub metadata: Vec<u8>,
    pub metadata_len: usize,
    pub generation: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ObjectStores {
    #[allow(clippy::type_complexity)]
    stores: Arc<RwLock<BTreeMap<ObjectStoreKey, BTreeMap<ObjectKey, ObjectValue>>>>,
}

impl ObjectStores {
    pub fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub(crate) fn store_exists(&self, obj_store_key: &str) -> Result<bool, ObjectStoreError> {
        Ok(self
            .stores
            .read()
            .map_err(|_| ObjectStoreError::PoisonedLock)?
            .get(&ObjectStoreKey::new(obj_store_key))
            .is_some())
    }

    pub fn lookup(
        &self,
        obj_store_key: &ObjectStoreKey,
        obj_key: &ObjectKey,
    ) -> Result<ObjectValue, ObjectStoreError> {
        self.stores
            .read()
            .map_err(|_| ObjectStoreError::PoisonedLock)?
            .get(obj_store_key)
            .and_then(|map| map.get(obj_key).cloned())
            .ok_or(ObjectStoreError::MissingObject)
    }

    pub(crate) fn insert_empty_store(
        &self,
        obj_store_key: ObjectStoreKey,
    ) -> Result<(), ObjectStoreError> {
        self.stores
            .write()
            .map_err(|_| ObjectStoreError::PoisonedLock)?
            .entry(obj_store_key)
            .and_modify(|_| {})
            .or_insert_with(BTreeMap::new);

        Ok(())
    }

    pub fn insert(
        &self,
        obj_store_key: ObjectStoreKey,
        obj_key: ObjectKey,
        obj: Vec<u8>,
        mode: KvInsertMode,
        generation: Option<u32>,
        metadata: Option<Vec<u8>>,
    ) -> Result<(), KvError> {
        // todo, handle mode and generation here
        // change ObjectStoreError to KvError, and impl into

        use std::time::SystemTime;

        let out_obj = match mode {
            KvInsertMode::Overwrite => {
                obj
            },
            KvInsertMode::Add => {
                let existing = self.lookup(&obj_store_key, &obj_key);
                if existing.is_ok() {
                    // key exists, add fails
                    return Err(KvError::PreconditionFailed)
                }
                obj
            },
            KvInsertMode::Append => {
                let existing = self.lookup(&obj_store_key, &obj_key);
                let mut out_obj;
                match existing {
                    Err(ObjectStoreError::MissingObject) => {
                        out_obj = obj;
                    },
                    Err(_) => return Err(KvError::InternalError),
                    Ok(mut v) => {
                        out_obj = obj;
                        out_obj.append(&mut v.body);
                    }
                }
                out_obj
            },
            KvInsertMode::Prepend => {
                let existing = self.lookup(&obj_store_key, &obj_key);
                let mut out_obj;
                match existing {
                    Err(ObjectStoreError::MissingObject) => {
                        out_obj = obj;
                    },
                    Err(_) => return Err(KvError::InternalError),
                    Ok(v) => {
                        out_obj = v.body;
                        out_obj.append(&mut obj.clone());
                    }
                }
                out_obj
            }
        };

        let mut obj_val = ObjectValue {
            body: out_obj,
            metadata: vec![],
            metadata_len: 0,
            generation: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u32,
        };

        if let Some(m) = metadata {
            obj_val.metadata_len = m.len();
            obj_val.metadata = m;
        }

        self.stores
            .write()
            .map_err(|_| KvError::InternalError)?
            .entry(obj_store_key)
            .and_modify(|store| {
                store.insert(obj_key.clone(), obj_val.clone());
            })
            .or_insert_with(|| {
                let mut store = BTreeMap::new();
                store.insert(obj_key, obj_val);
                store
            });

        Ok(())
    }

    pub fn delete(
        &self,
        obj_store_key: ObjectStoreKey,
        obj_key: ObjectKey,
    ) -> Result<(), ObjectStoreError> {
        self.stores
            .write()
            .map_err(|_| ObjectStoreError::PoisonedLock)?
            .entry(obj_store_key)
            .and_modify(|store| {
                store.remove(&obj_key);
            });

        Ok(())
    }

    pub fn list(&self, obj_store_key: &ObjectStoreKey) -> Result<Vec<Vec<u8>>, ObjectStoreError> {
        match self
            .stores
            .read()
            .map_err(|_| ObjectStoreError::PoisonedLock)?
            .get(obj_store_key)
        {
            None => Err(ObjectStoreError::UnknownObjectStore(
                obj_store_key.0.clone(),
            )),
            Some(s) => Ok(s
                .into_iter()
                .map(|(k, _)| k.0.as_bytes().to_vec())
                .collect()),
        }
    }
}

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Default)]
pub struct ObjectStoreKey(String);

impl ObjectStoreKey {
    pub fn new(key: impl ToString) -> Self {
        Self(key.to_string())
    }
}

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Default)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn new(key: impl ToString) -> Result<Self, KeyValidationError> {
        let key = key.to_string();
        is_valid_key(&key)?;
        Ok(Self(key))
    }
}

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, thiserror::Error)]
pub enum ObjectStoreError {
    #[error("The object was not in the store")]
    MissingObject,
    #[error("Viceroy's ObjectStore lock was poisoned")]
    PoisonedLock,
    /// An Object Store with the given name was not found.
    #[error("Unknown object-store: {0}")]
    UnknownObjectStore(String),
}

impl From<&ObjectStoreError> for FastlyStatus {
    fn from(e: &ObjectStoreError) -> Self {
        use ObjectStoreError::*;
        match e {
            MissingObject => FastlyStatus::None,
            PoisonedLock => panic!("{}", e),
            UnknownObjectStore(_) => FastlyStatus::Inval,
        }
    }
}

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, thiserror::Error)]
pub enum KvStoreError {
    #[error("The object was not in the store")]
    MissingObject,
    #[error("Viceroy's ObjectStore lock was poisoned")]
    PoisonedLock,
    /// An Object Store with the given name was not found.
    #[error("Unknown object-store: {0}")]
    UnknownObjectStore(String),
}

impl From<&KvStoreError> for ObjectStoreError {
    fn from(e: &KvStoreError) -> Self {
        use ObjectStoreError::*;
        match e {
            MissingObject        ,
            PoisonedLock         ,
            UnknownObjectStore(_),
        }
    }
}

impl From<&KvStoreError> for FastlyStatus {
    fn from(e: &KvStoreError) -> Self {
        use KvStoreError::*;
        match e {
            MissingObject => FastlyStatus::None,
            PoisonedLock => panic!("{}", e),
            UnknownObjectStore(_) => FastlyStatus::Inval,
        }
    }
}

/// Keys in the Object Store must follow the following rules:
///
///   * Keys can contain any sequence of valid Unicode characters, of length 1-1024 bytes when
///     UTF-8 encoded.
///   * Keys cannot contain Carriage Return or Line Feed characters.
///   * Keys cannot start with `.well-known/acme-challenge/`.
///   * Keys cannot be named `.` or `..`.
fn is_valid_key(key: &str) -> Result<(), KeyValidationError> {
    let len = key.as_bytes().len();
    if len < 1 {
        return Err(KeyValidationError::EmptyKey);
    } else if len > 1024 {
        return Err(KeyValidationError::Over1024Bytes);
    }

    if key.starts_with(".well-known/acme-challenge") {
        return Err(KeyValidationError::StartsWithWellKnown);
    }

    if key.eq("..") {
        return Err(KeyValidationError::ContainsDotDot);
    } else if key.eq(".") {
        return Err(KeyValidationError::ContainsDot);
    } else if key.contains('\r') {
        return Err(KeyValidationError::Contains("\r".to_owned()));
    } else if key.contains('\n') {
        return Err(KeyValidationError::Contains("\n".to_owned()));
    } else if key.contains('[') {
        return Err(KeyValidationError::Contains("[".to_owned()));
    } else if key.contains(']') {
        return Err(KeyValidationError::Contains("]".to_owned()));
    } else if key.contains('*') {
        return Err(KeyValidationError::Contains("*".to_owned()));
    } else if key.contains('?') {
        return Err(KeyValidationError::Contains("?".to_owned()));
    } else if key.contains('#') {
        return Err(KeyValidationError::Contains("#".to_owned()));
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum KeyValidationError {
    #[error("Keys for objects cannot be empty")]
    EmptyKey,
    #[error("Keys for objects cannot be over 1024 bytes in size")]
    Over1024Bytes,
    #[error("Keys for objects cannot start with `.well-known/acme-challenge`")]
    StartsWithWellKnown,
    #[error("Keys for objects cannot be named `.`")]
    ContainsDot,
    #[error("Keys for objects cannot be named `..`")]
    ContainsDotDot,
    #[error("Keys for objects cannot contain a `{0}`")]
    Contains(String),
}
