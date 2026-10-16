fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=../icons/app.ico");
    let _ = embed_resource::compile("app.rc", embed_resource::NONE);
}
