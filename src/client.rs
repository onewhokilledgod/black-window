use std::io;
use std::net::TcpStream;

use crate::connection;

pub fn run(address: &str) -> io::Result<()> {
	println!("Подключение к серверу: {address}");

	let stream = TcpStream::connect(address)?;

	println!("Подключение установлено!");

	connection::run(stream, "Сервер")
}
