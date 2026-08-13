//! Generates the control plane's gRPC client from the contract itself.
//!
//! `proto/control.proto` is a copy of the one the control plane serves.
//! Generating rather than hand-writing the messages is what makes drift a
//! compile error rather than a value quietly dropped on the floor.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/control.proto");
    // The server side is generated too, though this crate is a client: it is
    // what lets a test stand a fake control plane up and drive the real loop
    // against it.
    tonic_prost_build::configure().compile_protos(&["proto/control.proto"], &["proto"])?;
    Ok(())
}
