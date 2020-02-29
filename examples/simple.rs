use unix_ipc::{Bootstrapper, Receiver};

fn main() {
    let bootstrapper = Bootstrapper::new().unwrap();
    let path = bootstrapper.path().to_owned();

    let handle = std::thread::spawn(move || {
        let receiver = Receiver::<u32>::connect(path).unwrap();
        let a = receiver.recv().unwrap();
        let b = receiver.recv().unwrap();
        println!("{} + {} = {}", a, b, a + b);
    });

    bootstrapper.send(42u32).unwrap();
    bootstrapper.send(23u32).unwrap();

    handle.join().unwrap();
}
