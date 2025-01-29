use anyhow::Result;
use fjall::{Keyspace, PartitionCreateOptions, PartitionHandle, UserKey};
use uuid::Uuid;

use crate::activity_pub::model::{BaseObject, Create, Object};

use super::{ObjectRepo, make_object_key, uuidgen};

#[derive(Clone)]
pub(crate) struct OutboxKey {
    uid: String,
    sort_key: Uuid,
}

impl OutboxKey {
    fn new(uid: String) -> OutboxKey {
        OutboxKey {
            uid,
            sort_key: uuidgen(),
        }
    }
}

impl From<OutboxKey> for UserKey {
    fn from(value: OutboxKey) -> Self {
        let mut key = vec![];
        key.extend_from_slice(value.uid.as_bytes());
        key.extend_from_slice(value.sort_key.as_bytes());
        key.into()
    }
}

#[derive(Clone)]
pub(crate) struct OutboxIndex {
    keyspace: Keyspace,
    object_repo: ObjectRepo,
    iri_index: PartitionHandle,
    outbox_index: PartitionHandle,
}

impl OutboxIndex {
    pub(crate) fn new(keyspace: Keyspace) -> Result<OutboxIndex> {
        let object_repo = ObjectRepo::new(keyspace.clone())?;
        let options = PartitionCreateOptions::default();
        let iri_index = keyspace.open_partition("iri_index", options.clone())?;
        let outbox_index = keyspace.open_partition("outbox_index", options)?;
        Ok(OutboxIndex {
            keyspace,
            object_repo,
            iri_index,
            outbox_index,
        })
    }
    pub(crate) fn insert_create(&self, uid: String, act: Create) -> Result<()> {
        let mut batch = self.keyspace.batch();
        let obj = act.get_object();
        let act_iri = act.id().expect("activity should have IRI");
        let obj_iri = obj.id().expect("object should have IRI");
        let act_id = make_object_key();
        let obj_id = make_object_key();
        self.object_repo.batch_insert(&mut batch, act_id, act)?;
        self.object_repo.batch_insert(&mut batch, obj_id, obj)?;
        batch.insert(&self.iri_index, act_iri, act_id);
        batch.insert(&self.iri_index, obj_iri, obj_id);
        batch.insert(&self.outbox_index, OutboxKey::new(uid), act_id);
        batch.commit()?;
        Ok(())
    }
    // TODO pagination
    pub(crate) fn all(&self, uid: String) -> Result<Vec<Object>> {
        let mut keys = vec![];
        for pair in self.outbox_index.prefix(uid) {
            let (_, object_key) = pair?;
            keys.push(object_key);
        }
        let mut result = vec![];
        for key in keys {
            if let Some(obj) = self.object_repo.find_one(key)? {
                result.push(obj);
            }
        }
        Ok(result)
    }
}
