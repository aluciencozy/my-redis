use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use tokio::net::{TcpListener, TcpStream};

use crate::resp::{HandlerError, RespHandler};

mod commands;
mod resp;

enum Value {
    String(Vec<u8>),
    List(VecDeque<Vec<u8>>),
}

struct Entry {
    value: Value,
    expires_at: Option<Instant>,
}

type Store = Arc<Mutex<HashMap<Vec<u8>, Entry>>>;

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:6379").await.unwrap();
    let store: Store = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let store = Arc::clone(&store);

        tokio::spawn(async move {
            if let Err(err) = handle_connection(socket, store).await {
                eprintln!("connection error: {err}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, store: Store) -> Result<(), HandlerError> {
    let mut handler = RespHandler::new(stream);

    while let Some(request) = handler.read_value().await? {
        let reply = match commands::command_reply(request, &store) {
            Ok(Some(reply)) => reply,
            Ok(None) => continue,
            Err(error) => error.into_reply(),
        };

        handler.write_reply(&reply.encode()).await?
    }

    Ok(())
}
