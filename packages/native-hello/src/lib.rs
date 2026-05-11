use napi_derive::napi;

#[napi]
pub fn hello() -> String {
    "hello from native-hello".to_owned()
}
