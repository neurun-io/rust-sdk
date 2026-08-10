//! Generates the browser server's gRPC client from the contract itself.
//!
//! `proto/browser.proto` is a copy of the one `neurun-browser` serves. Building
//! the client from it rather than hand-writing messages is what makes drift a
//! compile error: a field added upstream lands in the generated struct, and the
//! exhaustive literal in `src/identity.rs` stops compiling until it is mapped.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/browser.proto");
    // The server side is generated too, though this crate is a client: it is
    // what lets a test stand a fake browser server up and drive the real loop
    // against it.
    tonic_prost_build::configure().compile_protos(&["proto/browser.proto"], &["proto"])?;
    Ok(())
}
