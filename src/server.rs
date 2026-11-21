use std::io;
use std::net::TcpListener;

use crate::connection;

pub fn run() -> io::Result<()> {
	let listener = TcpListener::bind("0.0.0.0:8080")?;

	println!("Ожидание подключения на порту 8080...");

	// Эта версия сервера обслуживает одного клиента за запуск.
	let (stream, address) = listener.accept()?;

	println!("Подключился клиент: {address}");

	connection::run(stream, "Клиент")
}
