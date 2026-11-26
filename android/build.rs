/// Имя, которое Яндекс показывает в списке устройств аккаунта.
const APP_NAME: &str = "Яндекс Музыка RS";

fn main() {
    println!("cargo:rustc-env=APP_NAME={APP_NAME}");
    // Общий код из ../src отличает телефон (Android и iOS) от десктопа по этому признаку.
    println!("cargo::rustc-check-cfg=cfg(mobile)");
    println!("cargo:rustc-cfg=mobile");
    slint_build::compile("ui/app.slint").unwrap();
}
