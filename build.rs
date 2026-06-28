fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("icons/icon.ico");
    res.compile().expect("Failed to compile icon resource");
}
