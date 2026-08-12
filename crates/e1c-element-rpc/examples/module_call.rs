use e1c_element_rpc::{ElementRpc, ModuleCallResponse};

fn main() -> Result<(), e1c_element_rpc::Error> {
    let rpc =
        ElementRpc::with_base_url("https://example.invalid")?.with_locale("en-US", "en_US")?;
    let request = rpc
        .call("e1c::example::Module", "Method")?
        .param("Std::String", "value")?
        .param("Example.Reference", "id")?
        .request()?;

    println!("{:?} {}", request.method(), request.url());
    println!("{}", request.body().unwrap_or_default());

    let response =
        ModuleCallResponse::<bool>::from_slice(r#"{"debugExitReason":null,"result":true}"#)?;
    println!("result={}", response.result()?);

    Ok(())
}
