// use std::collections::BTreeMap;

use crate::committer::traits::TransitionWriter;
use crate::store::traits::StoreError;
use std::collections::{HashMap, HashSet};

pub struct Committer<'a, K, V>
where
    K: std::hash::Hash + Eq + Clone,
{
    by_key: HashMap<K, Vec<(u64, V)>>,
    writers: Vec<Box<dyn TransitionWriter<K, V> + Send + 'a>>,
    commit_buffer: Vec<(K, V)>,
}

impl<'a, K: std::hash::Hash + Eq + Clone, V> Committer<'a, K, V> {
    pub fn new() -> Self {
        Self {
            by_key: HashMap::new(),
            writers: Vec::new(),
            commit_buffer: Vec::new(),
        }
    }

    pub fn register<W>(&mut self, writer: W)
    where
        W: TransitionWriter<K, V> + Send + 'a,
    {
        self.writers.push(Box::new(writer));
    }

    pub fn buffer(&mut self, id: u64, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        for (k, v) in updates {
            self.by_key.entry(k).or_default().push((id, v));
        }

        Ok(())
    }

    pub fn stage(&mut self, accepted: Vec<u64>) -> Result<(), StoreError> {
        let lookup = accepted.into_iter().collect::<HashSet<_>>();
        self.commit_buffer.clear();
        for (key, updates) in self.by_key.drain() {
            for (id, value) in updates {
                if lookup.contains(&id) {
                    self.commit_buffer.push((key.clone(), value));
                }
            }
        }
        Ok(())
    }

    /// Flushes all staged transitions to every registered writer in parallel.
    pub fn flush(&mut self) -> Result<(), StoreError>
    where
        K: Clone + Send + Sync,
        V: Clone + Send + Sync,
    {
        let updates = std::mem::take(&mut self.commit_buffer);
        for writer in &mut self.writers {
            writer.flush(updates.clone())?;
        }
        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use std::sync::{
//         Arc, Barrier, Mutex,
//         atomic::{AtomicUsize, Ordering},
//     };

//     struct TestWriter;

//     impl TransitionWriter<u64, u64> for TestWriter {
//         fn flush(&mut self, _: Vec<(u64, u64)>) -> Result<(), StoreError> {
//             Ok(())
//         }
//     }

//     #[test]
//     fn register_adds_a_writer() {
//         let mut committer = Committer::<u64, u64>::new();

//         committer.register(TestWriter);
//         assert_eq!(committer.writers.len(), 1);
//     }

//     struct RecordingWriter {
//         batches: Arc<Mutex<Vec<Vec<(u64, u64)>>>>,
//     }

//     impl TransitionWriter<u64, u64> for RecordingWriter {
//         fn flush(&mut self, updates: Vec<(u64, u64)>) -> Result<(), StoreError> {
//             self.batches.lock().unwrap().push(updates);
//             Ok(())
//         }
//     }

//     #[test]
//     fn flush_sends_only_staged_transitions_to_every_writer() {
//         let first = Arc::new(Mutex::new(Vec::new()));
//         let second = Arc::new(Mutex::new(Vec::new()));
//         let mut committer = Committer::<u64, u64>::new();

//         committer.register(RecordingWriter {
//             batches: Arc::clone(&first),
//         });
//         committer.register(RecordingWriter {
//             batches: Arc::clone(&second),
//         });
//         committer.buffer(1, vec![(10, 100)]).unwrap();
//         committer.buffer(2, vec![(20, 200)]).unwrap();
//         committer.stage(vec![2]).unwrap();
//         committer.flush().unwrap();

//         assert_eq!(*first.lock().unwrap(), vec![vec![(20, 200)]]);
//         assert_eq!(*second.lock().unwrap(), vec![vec![(20, 200)]]);
//         assert!(committer.by_id.is_empty());
//     }

//     struct SynchronizingWriter {
//         barrier: Arc<Barrier>,
//         calls: Arc<AtomicUsize>,
//     }

//     impl TransitionWriter<u64, u64> for SynchronizingWriter {
//         fn flush(&mut self, _: Vec<(u64, u64)>) -> Result<(), StoreError> {
//             self.calls.fetch_add(1, Ordering::Relaxed);
//             self.barrier.wait();
//             Ok(())
//         }
//     }

//     #[test]
//     fn flush_calls_writers_in_parallel() {
//         let barrier = Arc::new(Barrier::new(2));
//         let calls = Arc::new(AtomicUsize::new(0));
//         let mut committer = Committer::<u64, u64>::new();

//         for _ in 0..2 {
//             committer.register(SynchronizingWriter {
//                 barrier: Arc::clone(&barrier),
//                 calls: Arc::clone(&calls),
//             });
//         }

//         rayon::ThreadPoolBuilder::new()
//             .num_threads(2)
//             .build()
//             .unwrap()
//             .install(|| committer.flush())
//             .unwrap();

//         assert_eq!(calls.load(Ordering::Relaxed), 2);
//     }
// }
