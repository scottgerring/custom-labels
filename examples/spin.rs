use std::time::{Duration, Instant};

use custom_labels::writer;
use custom_labels::KeyHandle;

use rand::distributions::Alphanumeric;
use rand::Rng;

const KEY_L1: KeyHandle = KeyHandle::new(0);
const KEY_L2: KeyHandle = KeyHandle::new(1);

fn rand_str() -> String {
    String::from_utf8(
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn main() {
    // Initialize with 640 bytes (recommended max for eBPF readers)
    writer::setup(640);

    let mut last_update = Instant::now();

    loop {
        writer::with_attrs(
            [(KEY_L1, rand_str()), (KEY_L2, rand_str())],
            || loop {
                if last_update.elapsed() >= Duration::from_secs(10) {
                    break;
                }
            },
        );
        last_update = Instant::now();
    }
}
