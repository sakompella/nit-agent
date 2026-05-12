use napi_derive::napi;

#[napi]
pub fn hello() -> String {
    String::from("hello from native-hello")
}
