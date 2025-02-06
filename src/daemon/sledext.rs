use serde::{de::DeserializeOwned, Serialize};

use sled::Result;

pub trait KV: Serialize + DeserializeOwned {}
impl<T> KV for T where T: Serialize + DeserializeOwned {}

pub trait SledTreeExt {
    fn get_ext<K: KV, V: KV>(&self, k: &K) -> Result<Option<V>>;
    fn remove_ext<K: KV, V: KV>(&self, k: &K) -> Option<V>;
    fn first_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>>;
    fn pop_min_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>>;
    fn fetch_and_update_ext<K: KV, V: KV, F>(&self, k: &K, f: F) -> Result<Option<V>>
    where
        F: FnMut(Option<V>) -> Option<V>;
    fn insert_ext<K: KV, V: KV>(&self, k: &K, v: &V) -> Result<Option<V>>;
}

#[allow(clippy::unwrap_used)]
impl SledTreeExt for sled::Tree {
    fn get_ext<K: KV, V: KV>(&self, k: &K) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        Ok(self.get(k)?.map(|v| bincode::deserialize(&v).unwrap()))
    }

    fn remove_ext<K: KV, V: KV>(&self, key: &K) -> Option<V> {
        let key = bincode::serialize(key).unwrap();
        self.remove(key)
            .unwrap()
            .map(|value| bincode::deserialize(&value).unwrap())
    }

    fn first_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>> {
        let option = self.first()?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    fn pop_min_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>> {
        let option = self.pop_min()?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    fn fetch_and_update_ext<K: KV, V: KV, F>(&self, k: &K, mut f: F) -> Result<Option<V>>
    where
        F: FnMut(Option<V>) -> Option<V>,
    {
        let k = bincode::serialize(&k).unwrap();
        let f = |v: Option<&[u8]>| -> Option<Vec<u8>> {
            let v = v.map(|v| bincode::deserialize(v).unwrap());
            f(v).map(|v| bincode::serialize(&v).unwrap())
        };
        Ok(self
            .fetch_and_update(k, f)?
            .map(|v| bincode::deserialize::<V>(&v).unwrap()))
    }

    fn insert_ext<K: KV, V: KV>(&self, k: &K, v: &V) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        let v = bincode::serialize(v).unwrap();
        Ok(self
            .insert(k, v)?
            .map(|v| bincode::deserialize(&v).unwrap()))
    }
}
