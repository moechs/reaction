use std::{
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{spawn, JoinHandle},
};

type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct ThreadPool {
    workers: Vec<Worker>,
    sender: Sender<Job>,
}

impl ThreadPool {
    pub fn new(size: usize) -> ThreadPool {
        assert!(size > 0);

        let (sender, receiver) = channel();
        let receiver = Arc::new(Mutex::new(receiver));

        let mut workers = Vec::with_capacity(size);

        for _ in 0..size {
            workers.push(Worker::new(Arc::clone(&receiver)));
        }

        ThreadPool { workers, sender }
    }

    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let job = Box::new(f);

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        self.sender.send(job).unwrap();
    }

    pub fn join(self) {
        drop(self.sender);
        for worker in self.workers {
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            worker.thread.join().unwrap();
        }
    }
}

struct Worker {
    thread: JoinHandle<()>,
}

impl Worker {
    fn new(receiver: Arc<Mutex<Receiver<Job>>>) -> Worker {
        let thread = spawn(move || loop {
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            let received = receiver.lock().unwrap().recv();
            match received {
                Ok(job) => job(),
                Err(_) => break,
            }
        });

        Worker { thread }
    }
}
