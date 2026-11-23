//! Answers the guest's DNS queries. Only names the policy allows resolve, and
//! every answer is pinned so that later connections can be tied back to a name.

use crate::event::{EventKind, NetEvent};
use crate::gateway::{Shared, Upstream};
use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{RData, Record, RecordType};

const TTL: u32 = 60;

pub(crate) async fn answer<U: Upstream>(shared: &Shared<U>, query: &[u8]) -> Option<Vec<u8>> {
    let request = Message::from_vec(query).ok()?;
    let question = request.queries.first()?.clone();
    let name = question.name().to_utf8().trim_end_matches('.').to_ascii_lowercase();

    let mut response = Message::response(request.metadata.id, request.metadata.op_code);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.queries.push(question.clone());

    let verdict = shared.policy().check(&name, None);
    if !verdict.allowed {
        response.metadata.response_code = ResponseCode::NXDomain;
        // Clients ask for A and AAAA together; one log line per lookup is enough.
        if question.query_type() != RecordType::AAAA {
            shared.emit(NetEvent::new(EventKind::Dns, false, verdict.reason).host(&name));
        }
        return response.to_vec().ok();
    }

    // The guest is IPv4-only: every other record type gets an empty answer.
    if question.query_type() == RecordType::A {
        match shared.upstream.resolve(&name).await {
            Ok(ips) => {
                let ips = shared.pin(&name, ips);
                for ip in &ips {
                    response.add_answer(Record::from_rdata(question.name().clone(), TTL, RData::A(A(*ip))));
                }
                shared.emit(NetEvent::new(EventKind::Dns, true, verdict.reason).host(&name));
            }
            Err(err) => {
                response.metadata.response_code = ResponseCode::ServFail;
                shared.emit(NetEvent::new(EventKind::Dns, true, format!("lookup failed: {err}")).host(&name));
            }
        }
    }
    response.to_vec().ok()
}
