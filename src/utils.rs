/*
 * Copyright (C) 2023 Guillaume Pellegrino
 * This file is part of acsrs <https://github.com/guillaumepellegrino/acsrs>.
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
use crate::soap;
use bytes::Bytes;
use eyre::Result;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming as IncomingBody, Request, Response};
use native_tls::Certificate;
use openssl::x509::X509;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::io::Write;

pub fn req_path(req: &Request<IncomingBody>, num: u32) -> String {
    let mut i = 0;
    let mut split = req.uri().path().split('/');

    while i < num {
        split.next();
        i += 1;
    }

    match split.next() {
        Some(path) => String::from(path),
        None => String::from(""),
    }
}

pub fn reply(statuscode: u16, response: String) -> Result<Response<Full<Bytes>>> {
    let builder = Response::builder().status(statuscode);
    let reply = builder.body(Full::new(Bytes::from(response)))?;
    Ok(reply)
}

pub fn reply_xml(response: &soap::Envelope) -> Result<Response<Full<Bytes>>> {
    let text = quick_xml::se::to_string(&response)?;
    let builder = Response::builder()
        .header("User-Agent", "acsrs")
        .header("Content-type", "text/xml; charset=\"utf-8\"");
    let reply = builder.body(Full::new(Bytes::from(text)))?;
    Ok(reply)
}

pub fn reply_error(err: eyre::Report) -> Result<Response<Full<Bytes>>> {
    let reply = format!("Server internal error: {:?}\n", err);
    println!("{}", reply);
    Ok(Response::builder()
        .status(500)
        .body(Full::new(Bytes::from(reply)))?)
}

pub async fn content(req: &mut Request<IncomingBody>) -> Result<String> {
    let body = req.collect().await?.to_bytes();
    Ok(String::from_utf8(body.to_vec())?)
}

pub fn random_password() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(30)
        .map(char::from)
        .collect()
}

// Generate default certificates for a secure ACS connection with the specified CommonName
// Pipe acsrs/ssl/ssl.sh script into shell interpreter (cat acsrs/ssl/ssl.sh | bash)
pub fn gencertificates(acsdir: &std::path::Path, common_name: &str) {
    let bytes = include_bytes!("../ssl/ssl.sh");

    let mut child = std::process::Command::new("bash")
        .current_dir(acsdir)
        .env("CN", common_name)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(bytes).unwrap();
        drop(stdin);
    }

    let status = child.wait().unwrap().success();

    println!("status: {}", status);
}

pub fn get_cn(cert: Option<Certificate>) -> Option<String> {
    cert.and_then(
        |cert| match X509::from_der(&cert.to_der().unwrap_or_default()) {
            Ok(x509) => {
                println!("{:#?}", x509);
                Some(x509)
            }
            Err(err) => {
                eprintln!("Failed to parse certificate as x509: {err}");
                None
            }
        },
    )
    .and_then(|x509| {
        x509.subject_name()
            .entries()
            .next()
            .and_then(|name_entry| name_entry.data().as_utf8().ok())
            .map(|string| string.to_string())
    })
}
