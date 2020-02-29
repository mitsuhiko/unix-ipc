use serde_::{Deserialize, Serialize};
use std::env;
use std::process;
use unix_ipc::{channel, Bootstrapper, Receiver, Sender};

const ENV_VAR: &str = "PROC_CONNECT_TO";

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "serde_")]
pub enum Task {
    Sum(Vec<i64>, Sender<i64>),
    Shutdown,
}

fn main() {
    if let Ok(path) = env::var(ENV_VAR) {
        let receiver = Receiver::<Task>::connect(path).unwrap();
        loop {
            let task = receiver.recv().unwrap();
            match dbg!(task) {
                Task::Sum(values, tx) => {
                    tx.send(values.into_iter().sum::<i64>()).unwrap();
                }
                Task::Shutdown => break,
            }
        }
    } else {
        let bootstrapper = Bootstrapper::new().unwrap();
        let path = bootstrapper.path().to_owned();
        let mut child = process::Command::new(env::current_exe().unwrap())
            .env(ENV_VAR, path)
            .spawn()
            .unwrap();

        let (tx, rx) = channel().unwrap();
        bootstrapper.send(Task::Sum(vec![23, 42], tx)).unwrap();
        println!("result: {}", rx.recv().unwrap());

        let (tx, rx) = channel().unwrap();
        bootstrapper.send(Task::Sum(vec![1, 2, 3], tx)).unwrap();
        println!("result: {}", rx.recv().unwrap());

        bootstrapper.send(Task::Shutdown).unwrap();

        child.kill().ok();
        child.wait().ok();
    }
}
