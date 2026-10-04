use crate::reporting::{EvidenceTarget, ReportReader, ReportRequest, ReportView};
use std::{
    collections::BTreeMap,
    error::Error,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const INDEX: &[u8] = include_bytes!("dashboard.html");
const STYLES: &[u8] = include_bytes!("dashboard.css");
const SCRIPT: &[u8] = include_bytes!("dashboard.js");
const ICON: &[u8] = include_bytes!("dashboard-icon.svg");

pub fn serve() -> Result<(), Box<dyn Error>> {
    let reader = ReportReader::open_default_read_only()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    println!("Dashboard: http://{address}/ (Ctrl-C to stop)");
    for incoming in listener.incoming() {
        let mut stream = incoming?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let result = handle_connection(&mut stream, address, &reader);
        if let Err(error) = result {
            eprintln!("dashboard request failed: {error}");
        }
    }
    Ok(())
}

fn handle_connection(
    stream: &mut TcpStream,
    address: SocketAddr,
    reader: &ReportReader,
) -> io::Result<()> {
    let request = match read_request(stream) {
        Ok(request) => request,
        Err(_) => return respond(stream, 400, "text/plain; charset=utf-8", b"bad request"),
    };
    if !local_origin_allowed(&request.host, request.origin.as_deref(), address) {
        return respond(stream, 403, "text/plain; charset=utf-8", b"forbidden");
    }
    if request.method != "GET" {
        return respond(
            stream,
            405,
            "text/plain; charset=utf-8",
            b"method not allowed",
        );
    }

    let target = match request.target.as_str() {
        "/" => Some(("text/html; charset=utf-8", INDEX)),
        "/dashboard.css" => Some(("text/css; charset=utf-8", STYLES)),
        "/dashboard.js" => Some(("text/javascript; charset=utf-8", SCRIPT)),
        "/dashboard-icon.svg" => Some(("image/svg+xml", ICON)),
        _ => None,
    };
    if let Some((content_type, body)) = target {
        return respond(stream, 200, content_type, body);
    }
    let Some(query) = request.target.strip_prefix("/api/report?") else {
        return respond(stream, 404, "text/plain; charset=utf-8", b"not found");
    };
    let report_request = match parse_report_request(query) {
        Ok(request) => request,
        Err(_) => {
            return respond(
                stream,
                400,
                "text/plain; charset=utf-8",
                b"invalid report request",
            )
        }
    };
    match reader.query(&report_request) {
        Ok(snapshot) => respond_json(stream, 200, &snapshot),
        Err(error)
            if matches!(
                error.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::QueryReturnedNoRows)
            ) =>
        {
            respond(
                stream,
                404,
                "text/plain; charset=utf-8",
                b"report not found",
            )
        }
        Err(error) => {
            eprintln!("dashboard report query failed: {error}");
            respond(
                stream,
                500,
                "text/plain; charset=utf-8",
                b"report query failed",
            )
        }
    }
}

fn local_origin_allowed(host: &str, origin: Option<&str>, address: SocketAddr) -> bool {
    let host_is_listener = host.parse::<SocketAddr>().ok() == Some(address)
        || host
            .strip_prefix("localhost:")
            .and_then(|port| port.parse::<u16>().ok())
            == Some(address.port());
    if !host_is_listener {
        return false;
    }
    origin.is_none_or(|origin| origin.strip_prefix("http://") == Some(host))
}

struct HttpRequest {
    method: String,
    target: String,
    host: String,
    origin: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<HttpRequest> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0; 1024];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete request",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers too large",
            ));
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 2;
    let headers = std::str::from_utf8(&bytes[..end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid request headers"))?;
    let mut lines = headers.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or_default().to_owned();
    if request_line.next() != Some("HTTP/1.1") || !target.starts_with('/') || target.contains('#') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid request line",
        ));
    }
    let mut host = None;
    let mut origin = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid request header"))?;
        if name.eq_ignore_ascii_case("host") {
            if host.replace(value.trim().to_owned()).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate host header",
                ));
            }
        } else if name.eq_ignore_ascii_case("origin") {
            if origin.replace(value.trim().to_owned()).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate origin header",
                ));
            }
        } else if (name.eq_ignore_ascii_case("content-length") && value.trim() != "0")
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request body is not supported",
            ));
        }
    }
    Ok(HttpRequest {
        method,
        target,
        host: host
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing host header"))?,
        origin,
    })
}

fn parse_report_request(query: &str) -> Result<ReportRequest, Box<dyn Error>> {
    let params = parse_params(query)?;
    let view = required(&params, "view")?;
    let mut allowed = vec!["view", "limit", "offset"];
    let view = match view {
        "summary" => ReportView::Summary,
        "runs" => ReportView::Runs,
        "tasks" => ReportView::Tasks,
        "task" => {
            allowed.push("id");
            ReportView::Task(parse_required(&params, "id")?)
        }
        "run" => {
            allowed.push("id");
            ReportView::Run(parse_required(&params, "id")?)
        }
        "skills" => {
            allowed.push("inactivity_days");
            ReportView::Skills
        }
        "skill_matrix" => ReportView::SkillMatrix,
        "skill_calls" => {
            allowed.extend(["run_id", "skill"]);
            ReportView::SkillCalls {
                run_id: params
                    .get("run_id")
                    .map(|value| value.parse())
                    .transpose()?,
                skill: params.get("skill").cloned(),
            }
        }
        "tools" => ReportView::Tools,
        "tool" => {
            allowed.push("name");
            ReportView::Tool(required(&params, "name")?.to_owned())
        }
        "agent_models" => ReportView::AgentModels,
        "agent_skill_matrix" => ReportView::AgentSkillMatrix,
        "waste" => {
            allowed.extend(["inactivity_days", "repeat_count", "long_run_multiplier"]);
            ReportView::Waste
        }
        "evidence" => {
            allowed.extend(["kind", "id"]);
            let id = parse_required(&params, "id")?;
            ReportView::Evidence(match required(&params, "kind")? {
                "run" => EvidenceTarget::Run(id),
                "tool_call" => EvidenceTarget::ToolCall(id),
                "skill_call" => EvidenceTarget::SkillCall(id),
                "skill_inventory" => EvidenceTarget::Inventory(id),
                _ => return Err("unknown evidence kind".into()),
            })
        }
        "source_event" => {
            allowed.push("id");
            ReportView::SourceEvent(parse_required(&params, "id")?)
        }
        _ => return Err("unknown report view".into()),
    };
    if params.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("unexpected report parameter".into());
    }
    let mut request = ReportRequest::new(view);
    if let Some(value) = params.get("limit") {
        request.limit = value.parse()?;
    }
    if let Some(value) = params.get("offset") {
        request.offset = value.parse()?;
    }
    if let Some(value) = params.get("inactivity_days") {
        request.inactivity_days = value.parse()?;
    }
    if let Some(value) = params.get("repeat_count") {
        request.thresholds.repeat_count = value.parse()?;
        if request.thresholds.repeat_count < 2 {
            return Err("repeat_count must be at least 2".into());
        }
    }
    if let Some(value) = params.get("long_run_multiplier") {
        request.thresholds.long_run_multiplier = value.parse()?;
        if request.thresholds.long_run_multiplier < 2 {
            return Err("long_run_multiplier must be at least 2".into());
        }
    }
    Ok(request)
}

fn parse_params(query: &str) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut params = BTreeMap::new();
    if query.len() > MAX_REQUEST_BYTES {
        return Err("query too large".into());
    }
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = decode_component(name)?;
        let value = decode_component(value)?;
        if params.insert(name, value).is_some() {
            return Err("duplicate query parameter".into());
        }
    }
    Ok(params)
}

fn decode_component(value: &str) -> Result<String, Box<dyn Error>> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                let (Some(high), Some(low)) = (high, low) else {
                    return Err("invalid percent escape".into());
                };
                decoded.push((high * 16 + low) as u8);
                index += 3;
            }
            b'%' => return Err("incomplete percent escape".into()),
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    Ok(String::from_utf8(decoded)?)
}

fn required<'a>(
    params: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, Box<dyn Error>> {
    params
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {name}").into())
}

fn parse_required<T: std::str::FromStr>(
    params: &BTreeMap<String, String>,
    name: &str,
) -> Result<T, Box<dyn Error>>
where
    T::Err: Error + 'static,
{
    Ok(required(params, name)?.parse()?)
}

fn respond_json(
    stream: &mut TcpStream,
    status: u16,
    value: &impl serde::Serialize,
) -> io::Result<()> {
    let body = serde_json::to_vec(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    respond(stream, status, "application/json; charset=utf-8", &body)
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_routes_require_known_views_and_reject_ambiguous_parameters() {
        let request = parse_report_request("view=tool&name=read%2Ffile&limit=999").unwrap();
        assert!(matches!(request.view, ReportView::Tool(name) if name == "read/file"));
        assert_eq!(request.limit, 999);
        assert!(parse_report_request("view=run&id=1&id=2").is_err());
        assert!(parse_report_request("view=source_event&id=1&query=anything").is_err());
        assert!(parse_report_request("view=unknown").is_err());
        assert!(parse_report_request("view=tool&name=%GG").is_err());
    }

    #[test]
    fn dashboard_requests_cannot_lower_waste_thresholds() {
        assert!(parse_report_request("view=waste&repeat_count=1").is_err());
        assert!(parse_report_request("view=waste&long_run_multiplier=0").is_err());
    }
    #[test]
    fn dashboard_host_and_origin_must_match_the_loopback_listener() {
        let address = "127.0.0.1:43123".parse().unwrap();
        assert!(local_origin_allowed("127.0.0.1:43123", None, address));
        assert!(local_origin_allowed(
            "127.0.0.1:43123",
            Some("http://127.0.0.1:43123"),
            address
        ));
        assert!(!local_origin_allowed(
            "127.0.0.1:43123",
            Some("http://attacker.example"),
            address
        ));
        assert!(!local_origin_allowed(
            "attacker.example:43123",
            None,
            address
        ));
    }
}
