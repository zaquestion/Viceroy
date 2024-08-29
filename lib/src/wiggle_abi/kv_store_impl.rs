//! fastly_obj_store` hostcall implementations.

use super::types::{
    PendingKvDeleteHandle, PendingKvInsertHandle, PendingKvListHandle, PendingKvLookupHandle,
};
use crate::session::PeekableTask;
use crate::session::{
    PendingKvDeleteTask, PendingKvInsertTask, PendingKvListTask, PendingKvLookupTask,
};

use {
    crate::{
        body::Body,
        error::Error,
        object_store::{ObjectKey, ObjectStoreError},
        session::Session,
        wiggle_abi::{
            fastly_kv_store::FastlyKvStore,
            types::{BodyHandle, KvStoreHandle},
        },
    },
    wiggle::{GuestMemory, GuestPtr},
};

#[wiggle::async_trait]
impl FastlyKvStore for Session {
    fn open(
        &mut self,
        memory: &mut GuestMemory<'_>,
        name: GuestPtr<str>,
    ) -> Result<KvStoreHandle, Error> {
        let name = memory.as_str(name)?.ok_or(Error::SharedMemory)?;
        if self.object_store.store_exists(&name)? {
            self.obj_store_handle(&name)
        } else {
            Err(Error::ObjectStoreError(
                ObjectStoreError::UnknownObjectStore(name.to_owned()),
            ))
        }
    }

    async fn lookup(
        &mut self,
        memory: &mut GuestMemory<'_>,
        store: ObjectStoreHandle,
        key: GuestPtr<str>,
        opt_pending_body_handle_out: GuestPtr<PendingKvLookupHandle>,
    ) -> Result<(), Error> {
        let store = self.get_obj_store_key(store).unwrap();
        let key = ObjectKey::new(memory.as_str(key)?.ok_or(Error::SharedMemory)?.to_string())?;
        // just create a future that's already ready
        let fut = futures::future::ok(self.obj_lookup(store, &key));
        let task = PeekableTask::spawn(fut).await;
        memory.write(
            opt_pending_body_handle_out,
            self.insert_pending_kv_lookup(PendingKvLookupTask::new(task)),
        )?;
        Ok(())
    }

    async fn lookup_wait(
        &mut self,
        memory: &mut wiggle::GuestMemory<'_>,
        pending_body_handle: PendingKvLookupHandle,
        opt_body_handle_out: GuestPtr<BodyHandle>,
    ) -> Result<(), Error> {
        let pending_obj = self
            .take_pending_kv_lookup(pending_body_handle)?
            .task()
            .recv()
            .await?;
        // proceed with the normal match from lookup()
        match pending_obj {
            Ok(obj) => {
                let new_handle = self.insert_body(Body::from(obj));
                memory.write(opt_body_handle_out, new_handle)?;
                Ok(())
            }
            Err(ObjectStoreError::MissingObject) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn insert(
        &mut self,
        memory: &mut GuestMemory<'_>,
        store: ObjectStoreHandle,
        key: GuestPtr<str>,
        body_handle: BodyHandle,
        opt_pending_body_handle_out: GuestPtr<PendingKvInsertHandle>,
    ) -> Result<(), Error> {
        let store = self.get_obj_store_key(store).unwrap().clone();
        let key = ObjectKey::new(memory.as_str(key)?.ok_or(Error::SharedMemory)?.to_string())?;
        let bytes = self.take_body(body_handle)?.read_into_vec().await?;
        let fut = futures::future::ok(self.obj_insert(store, key, bytes));
        let task = PeekableTask::spawn(fut).await;
        memory.write(
            opt_pending_body_handle_out,
            self.insert_pending_kv_insert(PendingKvInsertTask::new(task)),
        )?;
        Ok(())
    }

    async fn insert_wait(
        &mut self,
        _memory: &mut GuestMemory<'_>,
        pending_insert_handle: PendingKvInsertHandle,
    ) -> Result<(), Error> {
        Ok((self
            .take_pending_kv_insert(pending_insert_handle)?
            .task()
            .recv()
            .await?)?)
    }

    async fn delete(
        &mut self,
        memory: &mut wiggle::GuestMemory<'_>,
        store: ObjectStoreHandle,
        key: GuestPtr<str>,
        opt_pending_delete_handle_out: GuestPtr<PendingKvDeleteHandle>,
    ) -> Result<(), Error> {
        let store = self.get_obj_store_key(store).unwrap().clone();
        let key = ObjectKey::new(memory.as_str(key)?.ok_or(Error::SharedMemory)?.to_string())?;
        let fut = futures::future::ok(self.obj_delete(store, key));
        let task = PeekableTask::spawn(fut).await;
        memory.write(
            opt_pending_delete_handle_out,
            self.insert_pending_kv_delete(PendingKvDeleteTask::new(task)),
        )?;
        Ok(())
    }

    async fn delete_wait(
        &mut self,
        _memory: &mut GuestMemory<'_>,
        pending_delete_handle: PendingKvDeleteHandle,
    ) -> Result<(), Error> {
        Ok((self
            .take_pending_kv_delete(pending_delete_handle)?
            .task()
            .recv()
            .await?)?)
    }

    async fn list(
        &mut self,
        memory: &mut GuestMemory<'_>,
        store: ObjectStoreHandle,
        opt_pending_body_handle_out: GuestPtr<PendingKvLookupHandle>,
    ) -> Result<(), Error> {
        let store = self.get_obj_store_key(store).unwrap();
        let key = ObjectKey::new(memory.as_str(key)?.ok_or(Error::SharedMemory)?.to_string())?;
        // just create a future that's already ready
        let fut = futures::future::ok(self.obj_lookup(store, &key));
        let task = PeekableTask::spawn(fut).await;
        memory.write(
            opt_pending_body_handle_out,
            self.insert_pending_kv_lookup(PendingKvLookupTask::new(task)),
        )?;
        Ok(())
    }

    async fn list_wait(
        &mut self,
        memory: &mut wiggle::GuestMemory<'_>,
        pending_body_handle: PendingKvLookupHandle,
        opt_body_handle_out: GuestPtr<BodyHandle>,
    ) -> Result<(), Error> {
        let pending_obj = self
            .take_pending_kv_lookup(pending_body_handle)?
            .task()
            .recv()
            .await?;
        // proceed with the normal match from lookup()
        match pending_obj {
            Ok(obj) => {
                let new_handle = self.insert_body(Body::from(obj));
                memory.write(opt_body_handle_out, new_handle)?;
                Ok(())
            }
            Err(ObjectStoreError::MissingObject) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}