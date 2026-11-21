use std::io::{self, BufRead};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc;
use std::thread;

use crate::protocol::{
	MAX_MESSAGE_SIZE, prepare_file, receive_messages, send_close, send_file,
	send_text,
};

enum Event {
	Input(String),
	InputClosed,
	InputError(io::Error),
	NetworkClosed(io::Result<()>),
}

pub fn run(stream: TcpStream, peer_name: &'static str) -> io::Result<()> {
	let reader_stream = stream.try_clone()?;

	let (event_sender, event_receiver) = mpsc::channel();

	// Поток получения данных из TCP-соединения.
	let network_sender = event_sender.clone();

	let receive_handle = thread::spawn(move || {
		let result = receive_messages(reader_stream, peer_name);

		let _ = network_sender.send(Event::NetworkClosed(result));
	});

	// Отдельный поток читает stdin.
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
					let _ = input_sender.send(Event::InputError(error));
					return;
				}
			}
		}

		let _ = input_sender.send(Event::InputClosed);
	});

	drop(event_sender);

	let mut writer = stream;

	// Здесь сохраняем критическую ошибку, чтобы вернуть её
	// вызывающему коду после завершения соединения.
	let mut connection_error: Option<io::Error> = None;

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

						connection_error = Some(error);
					}

					break;
				}

				if let Some(path) = message.strip_prefix("/file ") {
					let path = path.trim();

					if path.is_empty() {
						println!("Укажи путь к файлу: /file путь_к_файлу");

						continue;
					}

					// Локальные ошибки не затрагивают соединение.
					let prepared_file = match prepare_file(path) {
						Ok(file) => file,

						Err(error) => {
							eprintln!("Невозможно подготовить файл: {error}");

							continue;
						}
					};

					// Ошибка передачи может нарушить границы протокола.
					if let Err(error) = send_file(&mut writer, prepared_file) {
						eprintln!(
							"Ошибка передачи файла; соединение будет закрыто: {error}"
						);

						connection_error = Some(error);
						break;
					}
				} else if message.len() > MAX_MESSAGE_SIZE {
					// Ошибка пользовательского ввода:
					// соединение остаётся открытым.
					eprintln!(
						"Сообщение слишком длинное. Максимум: {MAX_MESSAGE_SIZE} байт"
					);

					continue;
				} else if let Err(error) = send_text(&mut writer, &message) {
					eprintln!(
						"Ошибка отправки сообщения; соединение будет закрыто: {error}"
					);

					connection_error = Some(error);
					break;
				}
			}

			Ok(Event::InputClosed) => {
				if let Err(error) = send_close(&mut writer) {
					eprintln!("Не удалось отправить пакет завершения: {error}");

					connection_error = Some(error);
				}

				break;
			}

			Ok(Event::InputError(error)) => {
				eprintln!("Ошибка чтения ввода: {error}");

				if let Err(close_error) = send_close(&mut writer) {
					eprintln!("Не удалось отправить пакет завершения: {close_error}");
				}

				connection_error = Some(error);
				break;
			}

			Ok(Event::NetworkClosed(Ok(()))) => {
				break;
			}

			Ok(Event::NetworkClosed(Err(error))) => {
				eprintln!("Ошибка получения данных: {error}");

				connection_error = Some(error);
				break;
			}

			Err(_) => {
				break;
			}
		}
	}

	// Разблокируем поток чтения и завершаем TCP-соединение.
	let _ = writer.shutdown(Shutdown::Both);

	if receive_handle.join().is_err() {
		eprintln!("Поток получения данных завершился с паникой");

		if connection_error.is_none() {
			connection_error = Some(io::Error::new(
				io::ErrorKind::Other,
				"Поток получения данных завершился с паникой",
			));
		}
	}

	println!("Соединение закрыто");

	match connection_error {
		Some(error) => Err(error),
		None => Ok(()),
	}
}
