use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const PACKET_TEXT: u8 = 0;
pub const PACKET_FILE: u8 = 1;
pub const PACKET_CLOSE: u8 = 2;

pub const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1 MiB
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MiB
pub const MAX_FILE_NAME_SIZE: usize = 255; // байт в UTF-8

const DOWNLOAD_DIRECTORY: &str = "downloads";

/// Подготовленный к отправке файл.
/// Все локальные проверки выполняются до начала передачи.
pub struct PreparedFile {
	file: File,
	file_name: String,
	file_size: u64,
}

/// Копирует данные порциями и показывает ход передачи в терминале.
/// Сетевой протокол не меняется: функция лишь заменяет io::copy.
fn copy_with_progress<R: Read, W: Write>(
	reader: &mut R,
	writer: &mut W,
	total_size: u64,
	operation: &str,
) -> io::Result<u64> {
	const BUFFER_SIZE: usize = 64 * 1024;

	let started_at = Instant::now();
	let mut buffer = [0u8; BUFFER_SIZE];
	let mut copied = 0u64;
	let mut last_percent = if total_size == 0 { 100 } else { 0 };

	// Сразу показываем начало передачи. Для пустого файла прогресс равен 100%.
	print_transfer_progress(operation, 0, total_size, started_at);

	loop {
		let bytes_read = match reader.read(&mut buffer) {
			Ok(0) => break,
			Ok(bytes_read) => bytes_read,
			Err(error) => {
				println!();
				return Err(error);
			}
		};

		if let Err(error) = writer.write_all(&buffer[..bytes_read]) {
			println!();
			return Err(error);
		}

		copied += bytes_read as u64;
		let percent = if total_size == 0 {
			100
		} else {
			(copied.saturating_mul(100) / total_size).min(100) as u8
		};

		if last_percent != percent {
			print_transfer_progress(operation, copied, total_size, started_at);
			last_percent = percent;
		}
	}

	println!();
	Ok(copied)
}

/// Обновляет одну строку прогресса, не засоряя терминал новыми строками.
fn print_transfer_progress(
	operation: &str,
	copied: u64,
	total_size: u64,
	started_at: Instant,
) {
	const BAR_WIDTH: usize = 24;

	let percent = if total_size == 0 {
		100
	} else {
		(copied.saturating_mul(100) / total_size).min(100) as usize
	};
	let filled = BAR_WIDTH * percent / 100;
	let bar = format!("{}{}", "#".repeat(filled), "-".repeat(BAR_WIDTH - filled));

	let elapsed_seconds = started_at.elapsed().as_secs_f64().max(0.001);
	let bytes_per_second = (copied as f64 / elapsed_seconds) as u64;

	let stdout = io::stdout();
	let mut output = stdout.lock();
	let _ = write!(
		output,
		"\r\x1B[2K{operation}: [{bar}] {percent:3}% | {} / {} | {}/s",
		format_bytes(copied),
		format_bytes(total_size),
		format_bytes(bytes_per_second),
	);
	let _ = output.flush();
}

/// Форматирует количество байтов в удобный для чтения вид.
fn format_bytes(bytes: u64) -> String {
	const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];

	let mut value = bytes as f64;
	let mut unit_index = 0;

	while value >= 1024.0 && unit_index < UNITS.len() - 1 {
		value /= 1024.0;
		unit_index += 1;
	}

	if unit_index == 0 {
		format!("{bytes} {}", UNITS[unit_index])
	} else {
		format!("{value:.1} {}", UNITS[unit_index])
	}
}

/// Отправляет текстовый пакет:
/// [тип: u8][длина: u32][содержимое].
pub fn send_text<W: Write>(writer: &mut W, message: &str) -> io::Result<()> {
	let bytes = message.as_bytes();

	if bytes.len() > MAX_MESSAGE_SIZE {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			format!("Сообщение превышает лимит в {MAX_MESSAGE_SIZE} байт"),
		));
	}

	let length = u32::try_from(bytes.len()).map_err(|_| {
		io::Error::new(
			io::ErrorKind::InvalidInput,
			"Размер сообщения не помещается в u32",
		)
	})?;

	writer.write_all(&[PACKET_TEXT])?;
	writer.write_all(&length.to_be_bytes())?;
	writer.write_all(bytes)?;

	Ok(())
}

/// Открывает файл и выполняет локальные проверки до передачи.
pub fn prepare_file(path: &str) -> io::Result<PreparedFile> {
	let file = File::open(path)?;
	let metadata = file.metadata()?;

	if !metadata.is_file() {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"Указанный путь не является обычным файлом",
		));
	}

	let file_size = metadata.len();

	if file_size > MAX_FILE_SIZE {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			format!("Файл превышает лимит в {MAX_FILE_SIZE} байт"),
		));
	}

	let file_name = Path::new(path)
		.file_name()
		.ok_or_else(|| {
			io::Error::new(io::ErrorKind::InvalidInput, "Некорректное имя файла")
		})?
		.to_string_lossy()
		.into_owned();

	validate_file_name(&file_name)?;

	Ok(PreparedFile {
		file,
		file_name,
		file_size,
	})
}

/// Отправляет файловый пакет:
/// [тип: u8][длина имени: u16][имя][размер: u64][содержимое].
pub fn send_file<W: Write>(
	writer: &mut W,
	prepared: PreparedFile,
) -> io::Result<()> {
	let PreparedFile {
		file,
		file_name,
		file_size,
	} = prepared;

	let name_bytes = file_name.as_bytes();

	let name_length = u16::try_from(name_bytes.len()).map_err(|_| {
		io::Error::new(io::ErrorKind::InvalidInput, "Имя файла слишком длинное")
	})?;

	writer.write_all(&[PACKET_FILE])?;
	writer.write_all(&name_length.to_be_bytes())?;
	writer.write_all(name_bytes)?;
	writer.write_all(&file_size.to_be_bytes())?;

	// Передаём строго file_size байтов.
	let mut limited_file = file.take(file_size);

	let sent =
		copy_with_progress(&mut limited_file, writer, file_size, "Отправка")?;

	if sent != file_size {
		return Err(io::Error::new(
			io::ErrorKind::UnexpectedEof,
			"Исходный файл изменился или закончился во время отправки",
		));
	}

	println!("Файл отправлен: {file_name} ({file_size} байт)");

	Ok(())
}

/// Отправляет пакет завершения:
/// [тип: u8].
pub fn send_close<W: Write>(writer: &mut W) -> io::Result<()> {
	writer.write_all(&[PACKET_CLOSE])
}

/// Читает входящие пакеты до отключения собеседника
/// или получения PACKET_CLOSE.
pub fn receive_messages<R: Read>(input: R, peer_name: &str) -> io::Result<()> {
	let mut reader = io::BufReader::new(input);

	loop {
		let mut packet_type = [0u8; 1];

		match reader.read_exact(&mut packet_type) {
			Ok(()) => {}

			Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
				println!("{peer_name} отключился");
				return Ok(());
			}

			Err(error) => return Err(error),
		}

		match packet_type[0] {
			PACKET_TEXT => receive_text(&mut reader),
			PACKET_FILE => receive_file(&mut reader),

			PACKET_CLOSE => {
				println!("{peer_name} завершил соединение");
				return Ok(());
			}

			_ => {
				return Err(io::Error::new(
					io::ErrorKind::InvalidData,
					"Получен неизвестный тип пакета",
				));
			}
		}?;
	}
}

fn receive_text<R: Read>(reader: &mut R) -> io::Result<()> {
	let mut length_bytes = [0u8; 4];

	reader.read_exact(&mut length_bytes)?;

	let length = u32::from_be_bytes(length_bytes) as usize;

	if length > MAX_MESSAGE_SIZE {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("Получено слишком большое сообщение: {length} байт"),
		));
	}

	let mut message_bytes = vec![0u8; length];

	reader.read_exact(&mut message_bytes)?;

	let message = String::from_utf8(message_bytes).map_err(|_| {
		io::Error::new(
			io::ErrorKind::InvalidData,
			"Текстовое сообщение содержит некорректный UTF-8",
		)
	})?;

	println!("Собеседник: {message}");

	Ok(())
}

fn receive_file<R: Read>(reader: &mut R) -> io::Result<()> {
	let mut name_length_bytes = [0u8; 2];

	reader.read_exact(&mut name_length_bytes)?;

	let name_length = u16::from_be_bytes(name_length_bytes) as usize;

	if name_length == 0 || name_length > MAX_FILE_NAME_SIZE {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			"Недопустимая длина имени файла",
		));
	}

	let mut name_bytes = vec![0u8; name_length];

	reader.read_exact(&mut name_bytes)?;

	let file_name = String::from_utf8(name_bytes).map_err(|_| {
		io::Error::new(
			io::ErrorKind::InvalidData,
			"Имя файла содержит некорректный UTF-8",
		)
	})?;

	validate_file_name(&file_name).map_err(|_| {
		io::Error::new(
			io::ErrorKind::InvalidData,
			"Получено недопустимое имя файла",
		)
	})?;

	let mut size_bytes = [0u8; 8];

	reader.read_exact(&mut size_bytes)?;

	let file_size = u64::from_be_bytes(size_bytes);

	if file_size > MAX_FILE_SIZE {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("Полученный файл превышает лимит в {MAX_FILE_SIZE} байт"),
		));
	}

	fs::create_dir_all(DOWNLOAD_DIRECTORY)?;

	// Создаём файл, не перезаписывая существующий.
	let (mut file, destination) = create_unique_file(&file_name)?;

	// Читаем ровно file_size байтов из потока.
	let copy_result = {
		let mut limited_reader = (&mut *reader).take(file_size);

		copy_with_progress(&mut limited_reader, &mut file, file_size, "Получение")
	};

	let received = match copy_result {
		Ok(received) => received,

		Err(error) => {
			drop(file);
			let _ = fs::remove_file(&destination);

			return Err(error);
		}
	};

	drop(file);

	if received != file_size {
		let _ = fs::remove_file(&destination);

		return Err(io::Error::new(
			io::ErrorKind::UnexpectedEof,
			"Файл получен не полностью",
		));
	}

	println!(
		"Получен файл: {} ({} байт)",
		destination.display(),
		file_size
	);

	Ok(())
}

fn validate_file_name(name: &str) -> io::Result<()> {
	let forbidden_characters = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

	if name.is_empty()
		|| name == "."
		|| name == ".."
		|| name.len() > MAX_FILE_NAME_SIZE
		|| name.ends_with('.')
		|| name.ends_with(' ')
		|| name.chars().any(|character| {
			character.is_control() || forbidden_characters.contains(&character)
		}) {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"Недопустимое имя файла",
		));
	}

	Ok(())
}

fn create_unique_file(file_name: &str) -> io::Result<(File, PathBuf)> {
	let original_path = Path::new(file_name);

	let stem = original_path
		.file_stem()
		.and_then(|value| value.to_str())
		.unwrap_or(file_name);

	let extension = original_path.extension().and_then(|value| value.to_str());

	for index in 0..10_000u32 {
		let candidate_name = if index == 0 {
			file_name.to_owned()
		} else if let Some(extension) = extension {
			format!("{stem} ({index}).{extension}")
		} else {
			format!("{file_name} ({index})")
		};

		let destination = Path::new(DOWNLOAD_DIRECTORY).join(candidate_name);

		match OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&destination)
		{
			Ok(file) => return Ok((file, destination)),

			Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
				continue;
			}

			Err(error) => return Err(error),
		}
	}

	Err(io::Error::new(
		io::ErrorKind::AlreadyExists,
		"Не удалось подобрать свободное имя для файла",
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Cursor;

	#[test]
	fn copy_with_progress_copies_data() {
		let input_bytes = vec![b'A'; 150 * 1024];
		let mut input = Cursor::new(input_bytes.clone());
		let mut output = Vec::new();

		let copied = copy_with_progress(
			&mut input,
			&mut output,
			input_bytes.len() as u64,
			"Тест",
		)
		.unwrap();

		assert_eq!(copied, input_bytes.len() as u64);
		assert_eq!(output, input_bytes);
	}

	#[test]
	fn copy_with_progress_handles_empty_input() {
		let mut input = Cursor::new(Vec::<u8>::new());
		let mut output = Vec::new();

		let copied =
			copy_with_progress(&mut input, &mut output, 0, "Тест").unwrap();

		assert_eq!(copied, 0);
		assert!(output.is_empty());
	}

	#[test]
	fn send_text_writes_correct_packet() {
		let mut output = Vec::new();
		let message = "Привет";

		send_text(&mut output, message).unwrap();

		let bytes = message.as_bytes();

		assert_eq!(output[0], PACKET_TEXT);

		assert_eq!(&output[1..5], &(bytes.len() as u32).to_be_bytes()[..]);

		assert_eq!(&output[5..], bytes);
	}

	#[test]
	fn send_text_rejects_oversized_message() {
		let mut output = Vec::new();
		let message = "A".repeat(MAX_MESSAGE_SIZE + 1);

		let error = send_text(&mut output, &message).unwrap_err();

		assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
		assert!(output.is_empty());
	}

	#[test]
	fn receive_messages_stops_on_close_packet() {
		let input = Cursor::new(vec![PACKET_CLOSE]);

		assert!(receive_messages(input, "Тест").is_ok());
	}

	#[test]
	fn receive_messages_rejects_unknown_packet() {
		let input = Cursor::new(vec![255]);

		let error = receive_messages(input, "Тест").unwrap_err();

		assert_eq!(error.kind(), io::ErrorKind::InvalidData);
	}

	#[test]
	fn receive_messages_rejects_truncated_text() {
		let mut packet = vec![PACKET_TEXT];

		packet.extend_from_slice(&3u32.to_be_bytes());
		packet.push(b'A');

		let input = Cursor::new(packet);

		let error = receive_messages(input, "Тест").unwrap_err();

		assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
	}
}
