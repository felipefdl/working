//! Compiles `proto/googlechat.proto` into the generated types included by `src/proto.rs`.

fn main() -> std::io::Result<()> {
  println!("cargo:rerun-if-changed=proto/googlechat.proto");
  prost_build::compile_protos(&["proto/googlechat.proto"], &["proto"])
}
