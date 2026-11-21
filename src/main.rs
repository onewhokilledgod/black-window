mod client;
mod connection;
mod protocol;
mod server;

use std::io;

fn main() -> io::Result<()> {
	let args: Vec<String> = std::env::args().collect();

	match args.get(1).map(String::as_str) {
		Some("server") => server::run(),
		Some("client") => {
			let address = args.get(2).map(String::as_str).unwrap_or("127.0.0.1:8080");

			client::run(address)
		}
		_ => {
			println!("Использование:");
			println!("  cargo run -- server");
			println!("  cargo run -- client 127.0.0.1:8080");
			println!();
			println!("Команды в чате:");
			println!("  /file путь_к_файлу — отправить файл");
			println!("  /quit               — завершить соединение");
			Ok(())
		}
	}
}
