use snafu::ResultExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::buffer::{BufferReader, BufferedReader};
use super::error::{FwdNetworkWriteWithNormalSnafu, Result};
use super::{CodecMessageReader, CodecMessageWriter, MessageReader, MessageWriter};
use crate::utils::codec::{Decryptor, Encryptor};

pub trait ForwardReader {
    async fn read(&mut self) -> Result<&'_ [u8]>;
}

pub trait ForwardWriter {
    async fn write(&mut self, src: &[u8]) -> Result<()>;
}

pub struct NormalForwardReader<'a, T> {
    buffered_reader: BufferReader<'a, T>,
}

impl<'a, T: AsyncReadExt + Unpin + Send> NormalForwardReader<'a, T> {
    pub fn new(reader: &'a mut T) -> Self {
        Self {
            buffered_reader: BufferReader::new(reader),
        }
    }
}

impl<'a, T: AsyncReadExt + Unpin + Send> ForwardReader for NormalForwardReader<'a, T> {
    async fn read(&mut self) -> Result<&'_ [u8]> {
        self.buffered_reader.read().await
    }
}

pub struct NormalForwardWriter<'a, T> {
    writer: &'a mut T,
}

impl<'a, T: AsyncWriteExt + Unpin + Send> NormalForwardWriter<'a, T> {
    pub fn new(writer: &'a mut T) -> Self {
        Self { writer }
    }

    async fn write_inner(&mut self, src: &[u8]) -> Result<()> {
        self.writer
            .write_all(src)
            .await
            .context(FwdNetworkWriteWithNormalSnafu)
    }
}

impl<'a, T: AsyncWriteExt + Unpin + Send> ForwardWriter for NormalForwardWriter<'a, T> {
    async fn write(&mut self, src: &[u8]) -> Result<()> {
        self.write_inner(src).await
    }
}

pub struct CodecForwardReader<'a, T: AsyncReadExt + Unpin + Send, D: Decryptor>(
    CodecMessageReader<'a, T, D>,
);

impl<'a, T: AsyncReadExt + Send + Unpin, D: Decryptor> CodecForwardReader<'a, T, D> {
    pub fn new(reader: &'a mut T, decryptor: D) -> Self {
        Self(CodecMessageReader::new(reader, decryptor))
    }
}

impl<'a, T: AsyncReadExt + Send + Unpin, D: Decryptor> ForwardReader
    for CodecForwardReader<'a, T, D>
{
    async fn read(&mut self) -> Result<&'_ [u8]> {
        self.0
            .read_msg()
            .await
            .map_err(|e| super::error::Error::MsgForward {
                action: "read",
                detail: format!("{}", snafu::Report::from_error(e)),
            })
    }
}

pub struct CodecForwardWriter<'a, T: AsyncWriteExt + Send + Unpin, E: Encryptor>(
    CodecMessageWriter<'a, T, E>,
);

impl<'a, T: AsyncWriteExt + Send + Unpin, E: Encryptor> CodecForwardWriter<'a, T, E> {
    pub fn new(writer: &'a mut T, encryptor: E) -> Self {
        Self(CodecMessageWriter::new(writer, encryptor))
    }
}

impl<'a, T: AsyncWriteExt + Send + Unpin, E: Encryptor> ForwardWriter
    for CodecForwardWriter<'a, T, E>
{
    /// SAFETY: Same as [`CodecMessageWriter`]
    async fn write(&mut self, src: &[u8]) -> Result<()> {
        self.0.write_msg(src).await
    }
}

pub async fn copy<R: ForwardReader, W: ForwardWriter>(
    mut reader: R,
    mut writer: W,
) -> Result<usize> {
    let mut length: usize = 0;
    loop {
        let src = reader.read().await?;
        let n = src.len();
        if n == 0 {
            return Ok(length);
        }
        writer.write(src).await?;
        length += n;
    }
}

pub async fn start_forward<
    ClientReader: ForwardReader,
    ClientWriter: ForwardWriter,
    ServerReader: ForwardReader,
    ServerWriter: ForwardWriter,
>(
    client_reader: ClientReader,
    client_writer: ClientWriter,
    server_reader: ServerReader,
    server_writer: ServerWriter,
) {
    let client_to_server = copy(client_reader, server_writer);
    let server_to_client = copy(server_reader, client_writer);
    tokio::select! {
        result = client_to_server =>{
            handle_forward_result( result,"client->server");
        },
        result = server_to_client =>{
            handle_forward_result( result,"server->client");
        }
    }
}

fn handle_forward_result(result: Result<usize>, detail: &'static str) {
    match result {
        Ok(len) => tracing::info!("forward finish! we send {len} bytes,detail:{detail}"),
        Err(e) => tracing::error!("got forward error:{e},detail:{detail}"),
    }
}

#[macro_export]
macro_rules! create_component {
    (Reader, $stream:expr,true, $key:expr, $get_codec:ident, $name:expr) => {
        CodecForwardReader::new(
            $stream,
            snafu_error_get_or_return_ok!(
                $get_codec(&$key),
                concat!("failed to create decoder when `", $name, "` forward msg")
            ),
        )
    };
    (Reader, $stream:expr,false, $key:expr, $get_codec:ident, $name:expr) => {
        NormalForwardReader::new($stream)
    };
    (Writer, $stream:expr,true, $key:expr, $get_codec:ident, $name:expr) => {
        CodecForwardWriter::new(
            $stream,
            snafu_error_get_or_return_ok!(
                $get_codec(&$key),
                concat!("failed to create encoder when `", $name, "` forward msg")
            ),
        )
    };
    (Writer, $stream:expr,false, $key:expr, $get_codec:ident, $name:expr) => {
        NormalForwardWriter::new($stream)
    };
}

/// When using it, please remember to manually import the following symbols:
/// - [`start_forward`]
/// - [`crate::create_component`]
/// - [`ForwardReader`]
/// - [`ForwardWriter`]
/// - [`CodecForwardReader`]
/// - [`CodecForwardWriter`]
/// - [`crate::snafu_error_get_or_return_ok`]
/// - [`super::get_decodec`]
/// - [`super::get_encodec`]
#[macro_export]
macro_rules! start_forward_with_codec_key {
    (
        $codec_key:expr,
        $client_reader:expr,
        $client_writer:expr,
        $server_reader:expr,
        $server_writer:expr,
        $client_reader_codec:tt,
        $client_writer_codec:tt,
        $server_reader_codec:tt,
        $server_writer_codec:tt
    ) => {
        match $codec_key {
            Some(key) => {
                (start_forward(
                    create_component!(
                        Reader,
                        $client_reader,
                        $client_reader_codec,
                        key,
                        get_decodec,
                        "client_reader"
                    ),
                    create_component!(
                        Writer,
                        $client_writer,
                        $client_writer_codec,
                        key,
                        get_encodec,
                        "client_writer"
                    ),
                    create_component!(
                        Reader,
                        $server_reader,
                        $server_reader_codec,
                        key,
                        get_decodec,
                        "server_reader"
                    ),
                    create_component!(
                        Writer,
                        $server_writer,
                        $server_writer_codec,
                        key,
                        get_encodec,
                        "server_writer"
                    ),
                )
                .await)
            }
            None => {
                (start_forward(
                    NormalForwardReader::new($client_reader),
                    NormalForwardWriter::new($client_writer),
                    NormalForwardReader::new($server_reader),
                    NormalForwardWriter::new($server_writer),
                )
                .await)
            }
        }
    };
}
