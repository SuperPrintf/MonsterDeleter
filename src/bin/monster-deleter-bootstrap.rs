#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The Explorer-facing entry point.  Legacy shell verbs are invoked once for
//! every selected item, so this tiny process never opens the overlay itself.
//! It only hands the received path to the mutex-protected selection broker.

use std::{
    env,
    net::UdpSocket,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    process::Command,
    thread,
    time::Duration,
};

const SELECTION_BROKER_ADDR: &str = "127.0.0.1:39618";
const SELECTION_BROKER_MAX_PACKET: usize = 60 * 1024;
const MAIN_EXE: &str = "monster-deleter.exe";

fn submit_selection(targets: &[PathBuf]) -> bool {
    let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else {
        return false;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(40)));

    // Explorer paths are UTF-16.  Preserve that representation end-to-end so
    // a path with non-ASCII characters is never lost in the hand-off.
    let mut payload = Vec::new();
    for target in targets {
        for unit in target.as_os_str().encode_wide() {
            payload.extend_from_slice(&unit.to_le_bytes());
        }
        payload.extend_from_slice(&0_u16.to_le_bytes());
    }
    if payload.len() > SELECTION_BROKER_MAX_PACKET
        || socket.send_to(&payload, SELECTION_BROKER_ADDR).is_err()
    {
        return false;
    }
    let mut acknowledgement = [0_u8; 1];
    socket
        .recv_from(&mut acknowledgement)
        .is_ok_and(|(length, _)| length == 1 && acknowledgement[0] == 1)
}

fn start_broker() {
    let Ok(exe) = env::current_exe() else {
        return;
    };
    let Some(directory) = exe.parent() else {
        return;
    };
    let _ = Command::new(directory.join(MAIN_EXE))
        .arg("--selection-broker")
        .spawn();
}

fn main() {
    let targets = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return;
    }

    // Start or wake the broker before the first submission.  Several bootstrap
    // instances may do this concurrently; the broker's named mutex grants
    // ownership to exactly one main process, while every other launch exits.
    // This lets all Explorer-spawned helpers submit within one collection pass.
    start_broker();
    for _ in 0..80 {
        if submit_selection(&targets) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}
