use {
    crate::wiggle_abi::types::{FastlyStatus, KvError, KvInsertMode},
    base64::prelude::*,
    serde::Serialize,
    std::{
        collections::BTreeMap,
        sync::{Arc, RwLock},
        time::SystemTime,
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
    ) -> Result<(), KvStoreError> {
        let existing = self.lookup(&obj_store_key, &obj_key);

        if let Some(g) = generation {
            if let Ok(val) = &existing {
                if val.generation != g {
                    return Err(KvStoreError::PreconditionFailed);
                }
            }
        }

        let out_obj = match mode {
            KvInsertMode::Overwrite => obj,
            KvInsertMode::Add => {
                if existing.is_ok() {
                    // key exists, add fails
                    return Err(KvStoreError::PreconditionFailed);
                }
                obj
            }
            KvInsertMode::Append => {
                let mut out_obj;
                match existing {
                    Err(ObjectStoreError::MissingObject) => {
                        out_obj = obj;
                    }
                    Err(_) => return Err(KvStoreError::InternalError),
                    Ok(v) => {
                        out_obj = v.body;
                        out_obj.append(&mut obj.clone());
                    }
                }
                out_obj
            }
            KvInsertMode::Prepend => {
                let mut out_obj;
                match existing {
                    Err(ObjectStoreError::MissingObject) => {
                        out_obj = obj;
                    }
                    Err(_) => return Err(KvStoreError::InternalError),
                    Ok(mut v) => {
                        out_obj = obj;
                        out_obj.append(&mut v.body);
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

        // magic number hack to ensure a case for integration tests
        if obj_val.generation == 1337 {
            obj_val.generation = 1338;
        }

        if let Some(m) = metadata {
            obj_val.metadata_len = m.len();
            obj_val.metadata = m;
        }

        self.stores
            .write()
            .map_err(|_| KvStoreError::InternalError)?
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
    ) -> Result<(), KvStoreError> {
        let mut res = Ok(());

        self.stores
            .write()
            .map_err(|_| KvStoreError::InternalError)?
            .entry(obj_store_key)
            .and_modify(|store| match store.get(&obj_key) {
                // 404 if the key doesn't exist, otherwise delete
                Some(_) => {
                    store.remove(&obj_key);
                }
                None => {
                    res = Err(KvStoreError::NotFound);
                }
            });

        res
    }

    pub fn list(
        &self,
        obj_store_key: ObjectStoreKey,
        cursor: Option<String>,
        prefix: Option<String>,
        limit: u32,
    ) -> Result<Vec<u8>, KvStoreError> {
        match self
            .stores
            .read()
            .map_err(|_| KvStoreError::InternalError)?
            .get(&obj_store_key)
        {
            None => Err(KvStoreError::InternalError),
            Some(s) => {
                let cursor = match cursor {
                    Some(c) => {
                        let cursor_bytes = BASE64_STANDARD
                            .decode(c)
                            .map_err(|_| KvStoreError::BadRequest)?;
                        let decoded = String::from_utf8(cursor_bytes)
                            .map_err(|_| KvStoreError::BadRequest)?;
                        Some(decoded)
                    }
                    None => None,
                };

                let mut list = s
                    .into_iter()
                    .filter(|(k, _)| {
                        if cursor.is_some() {
                            &k.0 > cursor.as_ref().unwrap()
                        } else {
                            true
                        }
                    })
                    .filter(|(k, _)| {
                        if prefix.is_some() {
                            k.0.starts_with(prefix.as_ref().unwrap())
                        } else {
                            true
                        }
                    })
                    .map(|(k, _)| String::from_utf8(k.0.as_bytes().to_vec()).unwrap())
                    .collect::<Vec<_>>();

                // limit
                let old_len = list.len();
                list.truncate(limit as usize);
                let new_len = list.len();

                let next_cursor = match old_len != new_len {
                    true => Some(BASE64_STANDARD.encode(&list[new_len - 1])),
                    false => None,
                };

                #[derive(Serialize)]
                struct Metadata {
                    limit: u32,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    prefix: Option<String>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    next_cursor: Option<String>,
                }
                #[derive(Serialize)]
                struct JsonOutput {
                    data: Vec<String>,
                    meta: Metadata,
                }

                let body = JsonOutput {
                    data: list,
                    meta: Metadata {
                        limit,
                        prefix,
                        next_cursor,
                    },
                };

                let some = serde_json::to_string(&body).map_err(|_| KvStoreError::InternalError)?;

                Ok(some.as_bytes().to_vec())
            }
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
    #[error("The error was not set")]
    Uninitialized,
    #[error("There was no error")]
    Ok,
    #[error("KV store cannot or will not process the request due to something that is perceived to be a client error")]
    BadRequest,
    #[error("KV store cannot find the requested resource")]
    NotFound,
    #[error("KV store cannot fulfill the request, as definied by the client's prerequisites (ie. if-generation-match)")]
    PreconditionFailed,
    #[error("The size limit for a KV store key was exceeded")]
    PayloadTooLarge,
    #[error("The system encountered an unexpected internal error")]
    InternalError,
    #[error("Too many requests have been made to the KV store")]
    TooManyRequests,
}

impl From<&KvError> for KvStoreError {
    fn from(e: &KvError) -> Self {
        match e {
            KvError::Uninitialized => KvStoreError::Uninitialized,
            KvError::Ok => KvStoreError::Ok,
            KvError::BadRequest => KvStoreError::BadRequest,
            KvError::NotFound => KvStoreError::NotFound,
            KvError::PreconditionFailed => KvStoreError::PreconditionFailed,
            KvError::PayloadTooLarge => KvStoreError::PayloadTooLarge,
            KvError::InternalError => KvStoreError::InternalError,
            KvError::TooManyRequests => KvStoreError::TooManyRequests,
        }
    }
}

impl From<&KvStoreError> for KvError {
    fn from(e: &KvStoreError) -> Self {
        match e {
            KvStoreError::Uninitialized => KvError::Uninitialized,
            KvStoreError::Ok => KvError::Ok,
            KvStoreError::BadRequest => KvError::BadRequest,
            KvStoreError::NotFound => KvError::NotFound,
            KvStoreError::PreconditionFailed => KvError::PreconditionFailed,
            KvStoreError::PayloadTooLarge => KvError::PayloadTooLarge,
            KvStoreError::InternalError => KvError::InternalError,
            KvStoreError::TooManyRequests => KvError::TooManyRequests,
        }
    }
}

impl From<&KvStoreError> for ObjectStoreError {
    fn from(e: &KvStoreError) -> Self {
        match e {
            // the only real one
            KvStoreError::NotFound => ObjectStoreError::MissingObject,
            _ => ObjectStoreError::UnknownObjectStore("".to_string()),
        }
    }
}

impl From<&KvStoreError> for FastlyStatus {
    fn from(e: &KvStoreError) -> Self {
        match e {
            KvStoreError::Uninitialized => panic!("{}", e),
            KvStoreError::Ok => FastlyStatus::Ok,
            KvStoreError::BadRequest => FastlyStatus::Inval,
            KvStoreError::NotFound => FastlyStatus::None,
            KvStoreError::PreconditionFailed => FastlyStatus::Inval,
            KvStoreError::PayloadTooLarge => FastlyStatus::Inval,
            KvStoreError::InternalError => FastlyStatus::Inval,
            KvStoreError::TooManyRequests => FastlyStatus::Inval,
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
