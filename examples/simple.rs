use serde_::{Deserialize, Serialize};
use unix_ipc::{
    channel, Bincode, BincodeReceiver as Receiver, BincodeSender as Sender, Bootstrapper,
};

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "serde_")]
pub enum Task {
    Sum(Vec<i64>, Sender<i64>),
    Shutdown,
}

fn main() {
    let bootstrapper: Bootstrapper<Bincode, _> = Bootstrapper::new().unwrap();
    let path = bootstrapper.path().to_owned();

    std::thread::spawn(move || {
        let receiver = Receiver::<Task>::connect(path).unwrap();
        loop {
            let task = receiver.recv().unwrap();
            match task {
                Task::Sum(values, tx) => {
                    tx.send(values.into_iter().sum::<i64>()).unwrap();
                }
                Task::Shutdown => break,
            }
        }
    });

    println!("make channel 1");
    let (tx, rx) = channel().unwrap();
    bootstrapper.send(Task::Sum(vec![23, 42], tx)).unwrap();
    println!("result: {}", rx.recv().unwrap());

    println!("make channel 2");
    let (tx, rx) = channel().unwrap();
    bootstrapper.send(Task::Sum(vec![1, 2, 3], tx)).unwrap();
    println!("result: {}", rx.recv().unwrap());

    bootstrapper.send(Task::Shutdown).unwrap();
}
