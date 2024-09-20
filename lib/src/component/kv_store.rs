use {
    super::fastly::api::{http_body, kv_store, types},
    super::types::TrappableError,
    crate::linking::ComponentCtx,
    crate::object_store::{ObjectKey, ObjectStoreError},
    crate::session::{
        PeekableTask, PendingKvDeleteTask, PendingKvInsertTask, PendingKvListTask,
        PendingKvLookupTask,
    },
    wasmtime_wasi::WasiView,
};

pub struct LookupResult {
    body: http_body::BodyHandle,
    metadata: Option<Vec<u8>>,
    generation: u32,
}

#[async_trait::async_trait]
impl kv_store::HostLookupResult for ComponentCtx {
    async fn body(
        &mut self,
        rep: wasmtime::component::Resource<kv_store::LookupResult>,
    ) -> wasmtime::Result<http_body::BodyHandle> {
        Ok(self.table().get(&rep)?.body)
    }

    async fn metadata(
        &mut self,
        rep: wasmtime::component::Resource<kv_store::LookupResult>,
        max_len: u64,
    ) -> Result<Option<Vec<u8>>, TrappableError> {
        let res = self.table().get(&rep)?;
        let Some(md) = res.metadata.as_ref() else {
            return Ok(None);
        };

        if md.len() > max_len as usize {
            return Err(types::Error::BufferLen(md.len() as u64).into());
        }

        Ok(self.table().get_mut(&rep)?.metadata.take())
    }

    async fn generation(
        &mut self,
        rep: wasmtime::component::Resource<kv_store::LookupResult>,
    ) -> wasmtime::Result<u32> {
        Ok(self.table().get(&rep)?.generation)
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<kv_store::LookupResult>,
    ) -> wasmtime::Result<()> {
        self.table().delete(rep)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl kv_store::Host for ComponentCtx {
    async fn open(&mut self, name: Vec<u8>) -> Result<Option<kv_store::Handle>, types::Error> {
        let name = String::from_utf8(name)?;
        if self.session.kv_store.store_exists(&name)? {
            // todo (byoung), handle optional/none/error case
            let h = self.session.kv_store_handle(&name)?;
            Ok(Some(h.into()))
        } else {
            Err(ObjectStoreError::UnknownObjectStore(name.to_owned()).into())
        }
    }

    async fn lookup(
        &mut self,
        store: kv_store::Handle,
        key: Vec<u8>,
    ) -> Result<kv_store::LookupHandle, types::Error> {
        let store = self.session.get_kv_store_key(store.into()).unwrap();
        let key = String::from_utf8(key)?;
        // just create a future that's already ready
        let fut = futures::future::ok(self.session.obj_lookup(store.clone(), ObjectKey::new(key)?));
        let task = PeekableTask::spawn(fut).await;
        let lh = self
            .session
            .insert_pending_kv_lookup(PendingKvLookupTask::new(task));
        Ok(lh.into())
    }

    async fn lookup_wait(
        &mut self,
        _handle: kv_store::LookupHandle,
    ) -> Result<
        (
            Option<wasmtime::component::Resource<kv_store::LookupResult>>,
            kv_store::KvStatus,
        ),
        types::Error,
    > {
        todo!()
    }

    async fn insert(
        &mut self,
        _store: kv_store::Handle,
        _key: Vec<u8>,
        _body_handle: kv_store::BodyHandle,
        _mask: kv_store::InsertConfigOptions,
        _config: kv_store::InsertConfig,
    ) -> Result<kv_store::InsertHandle, types::Error> {
        todo!()
    }

    async fn insert_wait(
        &mut self,
        _handle: kv_store::InsertHandle,
    ) -> Result<kv_store::KvStatus, types::Error> {
        todo!()
    }

    async fn delete(
        &mut self,
        _store: kv_store::Handle,
        _key: Vec<u8>,
    ) -> Result<kv_store::DeleteHandle, types::Error> {
        todo!()
    }

    async fn delete_wait(
        &mut self,
        _handle: kv_store::DeleteHandle,
    ) -> Result<kv_store::KvStatus, types::Error> {
        todo!()
    }

    async fn list(
        &mut self,
        _store: kv_store::Handle,
        _mask: kv_store::ListConfigOptions,
        _options: kv_store::ListConfig,
    ) -> Result<kv_store::ListHandle, types::Error> {
        todo!()
    }

    async fn list_wait(
        &mut self,
        _handle: kv_store::ListHandle,
    ) -> Result<(Option<kv_store::BodyHandle>, kv_store::KvStatus), types::Error> {
        todo!()
    }
}
