use super::{Result, missing};
use crate::http_support::Body;
use http::{HeaderValue, Response, StatusCode, header};

include!(concat!(env!("OUT_DIR"), "/admin_assets.rs"));

pub(super) fn serve(path: &str) -> Result<Response<Body>> {
    let (mime, bytes) = embedded(path).ok_or_else(missing)?;
    let mut response = crate::http_support::response(StatusCode::OK, bytes);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    Ok(response)
}
