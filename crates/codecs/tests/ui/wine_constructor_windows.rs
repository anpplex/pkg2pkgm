use pkg2mpkg_codecs::ResourceCompilerBackend;

fn main() {
    let _ = ResourceCompilerBackend::wine("resourcecompiler64.exe", "wine", "winepath");
}
