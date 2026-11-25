fn main() {
    contract::build_rust(std::path::Path::new("../app.contract")).expect("contract rust");
}
