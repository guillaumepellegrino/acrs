use std::sync::{Arc};
use std::collections::VecDeque;
use std::collections::HashMap;
use bytes::Bytes;
use http_body_util::Full;
use tokio;
use tokio::sync::{RwLock, mpsc};
use hyper::{body::Incoming as IncomingBody, Request, Response};
use rand::{thread_rng, Rng};
use rand::distributions::Alphanumeric;
use base64::Engine;
use crate::acs::{*};
use crate::soap;
use crate::utils;

pub struct Session {
    acs: Arc<RwLock<Acs>>,
    cpe: Option<Arc<RwLock<CPE>>>,
    observer: Option<mpsc::Sender<soap::Envelope>>,
    id: u32,
}

impl Session {
    pub fn new(acs: Arc<RwLock<Acs>>) -> Self {
        Session {
            acs: acs.clone(),
            cpe: None,
            observer: None,
            id: 0,
        }
    }

    async fn cpe_handle_inform(self: &mut Self, inform: &soap::Inform) {
        let connreq_url = match inform.parameter_list.get_value("Device.ManagementServer.ConnectionRequestURL") {
            Some(value) => value,
            None => {return;},
        };

        let mut acs = self.acs.write().await;
        let cpelock = acs.cpe_list.entry(inform.device_id.serial_number.clone()).or_default();
        self.cpe = Some(cpelock.clone());
        let mut cpe = cpelock.write().await;
        cpe.device_id = inform.device_id.clone();
        if cpe.connreq.url != connreq_url {
            println!("connreq.url is not configred: Configure ConnectionRequest");
            cpe.connreq.url = String::from(connreq_url);

            // Push a tranfer to configure ConnectionRequest with an SPV
            let mut transfer = Transfer::new();
            let spv = transfer.msg.add_spv(1);
            spv.push(soap::ParameterValue::new(
                "Device.ManagementServer.ConnectionRequestUsername", "xsd:string", &cpe.connreq.username));
            spv.push(soap::ParameterValue::new(
                "Device.ManagementServer.ConnectionRequestPassword", "xsd:string", &cpe.connreq.password));
            cpe.transfers.push_back(transfer);
        }
    }

    async fn cpe_check_transfers(self: &mut Self) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error>> {
        let mut cpe = match &self.cpe {
            Some(cpe) => cpe.write().await,
            None => {
                println!("Unknown CPE: Reply with no content");
                return utils::reply(204, String::from(""));
            }
        };

        let mut transfer = match cpe.transfers.pop_front() {
            Some(transfer) => transfer,
            None => {
                println!("No pending transfer for CPE: Reply with no content");
                return utils::reply(204, String::from(""));
            }
        };
        transfer.msg.header.id.text = self.id;
        self.observer = transfer.observer;
        drop(cpe);

        println!("Transfer pending message");
        return utils::reply_xml(&transfer.msg);
    }

    async fn handle_cpe_request(self: &mut Self, req: &mut Request<IncomingBody>) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error>> {
        println!("Received from CPE:");
        println!("  Headers: {:?}", req.headers());

        let content = utils::content(req).await?;
        //println!("  Content: [\n{}\n]", content);

        //  The CPE has nothing to tell us: We may ask for a new Transfer
        if content.is_empty() {
            return self.cpe_check_transfers().await;
        }

        // Try to parse XML content
        let envelope: soap::Envelope = match quick_xml::de::from_str(&content) {
            Ok(value) => value,
            Err(e) => {
                println!("Failed to parse XML: {:?}", e);
                println!("Content: [\n{}\n]\nContent End", content);
                return utils::reply(204, String::from(""));
            },
        };

        // Process message sent by CPE
        if let Some(inform) = envelope.inform() {
            println!("Inform received from {}", inform.device_id.serial_number);
            println!("Session ID: {}", envelope.id());
            self.id = envelope.id();

            for event in &inform.event.event_struct {
                println!("Event: {}", event.event_code);
            }
            self.cpe_handle_inform(&inform).await;

            let mut response = soap::Envelope::new(envelope.id());
            response.add_inform_response();
            return utils::reply_xml(&response);
        }
        else if let Some(_gpv_response) = envelope.body.gpv_response.first() {
            println!("GPV Response");
        }
        else if let Some(spv_response) = envelope.body.spv_response.first() {
            println!("SPV Response: {}", spv_response.status);
        }
        else {
            println!("Unknown SOAP/xml request: {}", content);
            return utils::reply(204, String::from(""));
        }

        // If someone if observing the current Transfer,
        // we forward the CPE response to him.
        if let Some(observer) = self.observer.as_mut() {
            if let Err(err) = observer.send(envelope).await {
                println!("Failed to forward Response: {:?}", err);
            }
            self.observer = None;
        }

        // We may ask for a new Transfer
        return self.cpe_check_transfers().await;
    }

    async fn check_cpe_authorization(self: &mut Self, req: &mut Request<IncomingBody>) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error>> {

        match req.headers().get("authorization") {
            Some(wwwauth) => {
                let wwwauth = wwwauth.to_str()?;
                if wwwauth == &self.acs.read().await.basicauth {
                    self.handle_cpe_request(req).await
                }
                else {
                    utils::reply(403, String::from("Forbidden\n"))
                }
            }
            None => {
                println!("auth required!");
                let response = String::from("Authorization required\n");
                let builder = Response::builder()
                    .header("User-Agent", "acsrs")
                    .header("WWW-Authenticate", "Basic realm=\"acrsrs world\"")
                    .status(401);
                Ok(builder.body(Full::new(Bytes::from(response)))?)
            }
        }
    }


    pub async fn handle(self: &mut Self, req: &mut Request<IncomingBody>) -> Result<Response<Full<Bytes>>, hyper::Error> {
        let reply = match req.uri().path() {
            "/cwmpWeb/CPEMgt" => self.check_cpe_authorization(req).await,
            _                 => utils::reply(403, String::from("Forbidden\n")),
        };

        match reply {
            Ok(reply)  => Ok(reply),
            Err(error) => utils::reply_error(error),
        }
    }
}

