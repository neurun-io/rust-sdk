//! Generates the control plane's gRPC clients from the contracts themselves.
//!
//! Each file under `proto/` is a copy of one the control plane serves.
//! Generating rather than hand-writing the messages is what makes drift a
//! compile error rather than a value quietly dropped on the floor.

const CONTRACTS: [&str; 3] = [
    "proto/browser.proto",
    "proto/document.proto",
    "proto/memory.proto",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for contract in CONTRACTS {
        println!("cargo:rerun-if-changed={contract}");
    }
    // The server side is generated too, though this crate is a client: it is
    // what lets a test stand a fake control plane up and drive the real loop
    // against it.
    tonic_prost_build::configure().compile_protos(&CONTRACTS, &["proto"])?;
    Ok(())
}
