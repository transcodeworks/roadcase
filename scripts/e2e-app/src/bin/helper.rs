//! The helper program the fixture application carries beside its executable,
//! signed on macOS with entitlements of its own.

fn main() {
    println!("helper {}", env!("FIXTURE_VERSION"));
}
