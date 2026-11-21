use std::io::{self, BufRead};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc;
use std::thread;

use crate::protocol::{
	prepare_file, receive_messages, send_close, send_file, send_text,
};

enum Event {
	Input(String),
	InputClosed,
	InputError(String),
	NetworkClosed(Result<(), String>),
}

pub fn run(stream: TcpStream, peer_name: &'static str) -> io::Result<()> {
	let reader_stream = stream.try_clone()?;
	let (event_sender, event_receiver) = mpsc::channel();

	// Поток получения данных из TCP-соединения.
	let network_sender = event_sender.clone();
	let receive_handle = thread::spawn(move || {
		let result = receive_messages(reader_stream, peer_name)
			.map_err(|error| error.to_string());
		let _ = network_sender.send(Event::NetworkClosed(result));
	});

	// Отдельный поток читает stdin, чтобы основной поток мог заметить отключение собеседника.
	let input_sender = event_sender.clone();
	thread::spawn(move || {
		let stdin = io::stdin();

		for line in stdin.lock().lines() {
			match line {
				Ok(line) => {
					if input_sender.send(Event::Input(line)).is_err() {
						return;
					}
				}
				Err(error) => {
					let _ = input_sender.send(Event::InputError(error.to_string()));
					return;
				}
			}
		}

		let _ = input_sender.send(Event::InputClosed);
	});

	drop(event_sender);

	let mut writer = stream;

	println!();
	println!("Чат запущен!");
	println!("Отправка файла: /file путь_к_файлу");
	println!("Завершение чата: /quit");

	loop {
		match event_receiver.recv() {
			Ok(Event::Input(message)) => {
				if message == "/quit" {
					if let Err(error) = send_close(&mut writer) {
						eprintln!("Не удалось отправить пакет завершения: {error}");
					}
					break;
				}

				if let Some(path) = message.strip_prefix("/file ") {
					let path = path.trim();

					if path.is_empty() {
						println!("Укажи путь к файлу: /file путь_к_файлу");
						continue;
					}

					// Локальные ошибки (например, файл не найден) не затрагивают соединение.
					let prepared_file = match prepare_file(path) {
						Ok(file) => file,
						Err(error) => {
							eprintln!("Невозможно подготовить файл: {error}");
							continue;
						}
					};

					// Ошибка после начала записи пакета может нарушить границы протокола.
					if let Err(error) = send_file(&mut writer, prepared_file) {
						eprintln!(
							"Ошибка передачи файла; соединение будет закрыто: {error}"
						);
						break;
					}
				} else if message.len() > crate::protocol::MAX_MESSAGE_SIZE {
					eprintln!("Сообщение слишком длинное");
					continue;
				} else if let Err(error) = send_text(&mut writer, &message) {
					eprintln!(
						"Ошибка отправки сообщения; соединение будет закрыто: {error}"
					);
					break;
				}
			}
			Ok(Event::InputClosed) => {
				let _ = send_close(&mut writer);
				break;
			}
			Ok(Event::InputError(error)) => {
				eprintln!("Ошибка чтения ввода: {error}");
				let _ = send_close(&mut writer);
				break;
			}
			Ok(Event::NetworkClosed(Ok(()))) => break,
			Ok(Event::NetworkClosed(Err(error))) => {
				eprintln!("Ошибка получения данных: {error}");
				break;
			}
			Err(_) => break,
		}
	}

	// Разблокируем поток чтения и завершаем TCP-соединение в обе стороны.
	let _ = writer.shutdown(Shutdown::Both);

	if receive_handle.join().is_err() {
		eprintln!("Поток получения данных завершился с паникой");
	}

	println!("Соединение закрыто");

	Ok(())
}
