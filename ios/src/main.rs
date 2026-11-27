//! Яндекс Музыка RS для iOS. Вся логика в общей с Android библиотеке (android/src/lib.rs),
//! здесь только точка входа: winit сам запускает UIApplicationMain на главном потоке.

fn main() {
    yamusic::ios_main();
}
