use hidapi::HidApi;

fn main() {
    let api = HidApi::new().expect("initialize HIDAPI");
    let devices = api
        .device_list()
        .map(|device| {
            format!(
                "{:04x}:{:04x}:{}:{}",
                device.vendor_id(),
                device.product_id(),
                device.usage_page(),
                device.usage()
            )
        })
        .collect::<Vec<_>>();
    println!("devices={} {}", devices.len(), devices.join(","));
}
