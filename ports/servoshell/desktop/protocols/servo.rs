/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Loads resources using a mapping from well-known shortcuts to resource: urls.
//! Recognized shortcuts:
//! - servo:default-user-agent
//! - servo:experimental-preferences
//! - servo:config
//! - servo:newtab
//! - servo:newtab-edit (Sirocco: add or remove a new tab shortcut)
//! - servo:welcome (Sirocco)
//! - servo:known-issues (Sirocco)
//! - servo:history (Sirocco: browsing history; never opened by web pages)
//! - servo:preferences

use std::future::Future;
use std::pin::Pin;

use headers::{ContentType, HeaderMapExt};
use servo::UserAgentPlatform;
use servo::protocol_handler::{
    Destination, DoneChannel, FetchContext, NetworkError, Origin, ProtocolHandler, Request,
    RequestMode, ResourceFetchTiming, Response, ResponseBody,
};

use crate::desktop::protocols::resource::ResourceProtocolHandler;
use crate::prefs::EXPERIMENTAL_PREFS;

#[derive(Default)]
pub struct ServoProtocolHandler {}

impl ProtocolHandler for ServoProtocolHandler {
    fn privileged_paths(&self) -> &'static [&'static str] {
        &["config", "preferences"]
    }

    fn is_fetchable(&self) -> bool {
        true
    }

    fn load(
        &self,
        request: &mut Request,
        done_chan: &mut DoneChannel,
        context: &FetchContext,
    ) -> Pin<Box<dyn Future<Output = Response> + Send>> {
        let url = request.current_url();

        if !allowed(request, url.path()) {
            return Box::pin(std::future::ready(Response::network_error(
                NetworkError::ResourceLoadError("Not available to web pages".to_owned()),
            )));
        }

        match url.path() {
            "config" => ResourceProtocolHandler::response_for_path(
                request,
                done_chan,
                context,
                "/config.html",
            ),
            // Sirocco: the new-tab page is generated so its search box uses the chosen engine.
            "newtab" => html_response(request, crate::pteron::newtab_html()),
            "newtab-edit" => html_response(request, crate::pteron::newtab_edit(url.as_url())),
            "welcome" => html_response(request, crate::pteron::welcome_html()),
            "compat" => html_response(request, crate::compat_view::placeholder_html(url.as_url())),
            "known-issues" => html_response(request, crate::pteron::known_issues_html()),
            "history" => html_response(request, crate::library::history_html(url.as_url())),
            "http-warning" => html_response(request, crate::secure::warning_html(url.as_url())),
            "http-continue" => html_response(request, crate::secure::continue_html(url.as_url())),
            "danger-continue" => html_response(request, crate::danger::continue_html(url.as_url())),

            "preferences" => ResourceProtocolHandler::response_for_path(
                request,
                done_chan,
                context,
                "/preferences.html",
            ),

            "license" => ResourceProtocolHandler::response_for_path(
                request,
                done_chan,
                context,
                "/license.html",
            ),

            "experimental-preferences" => {
                let pref_list = EXPERIMENTAL_PREFS
                    .iter()
                    .map(|pref| format!("\"{pref}\""))
                    .collect::<Vec<String>>()
                    .join(",");
                json_response(request, format!("[{pref_list}]"))
            },

            "default-user-agent" => {
                let user_agent = UserAgentPlatform::default().to_user_agent_string();
                json_response(request, format!("\"{user_agent}\""))
            },

            _ => Box::pin(std::future::ready(Response::network_error(
                NetworkError::ResourceLoadError("Invalid shortcut".to_owned()),
            ))),
        }
    }
}

/// Sirocco: what may load a `servo:` page. These pages hold private things (the new tab page lists
/// shortcuts, bookmarks and most visited sites, and secrets that let it change them), so:
/// - no web page may read one (fetch, XHR); the config page may read the two plain answers it asks for;
/// - no page may put one in a frame;
/// - a web page may open only the harmless ones in a tab; the rest open only from Sirocco itself
///   or from another `servo:` page (whose origin is opaque).
fn allowed(request: &Request, path: &str) -> bool {
    let from_web_page = matches!(request.origin, Origin::Origin(_)) && !request.origin.is_opaque();
    if request.mode != RequestMode::Navigate {
        return !from_web_page && matches!(path, "experimental-preferences" | "default-user-agent");
    }
    if request.destination != Destination::Document {
        return false;
    }
    !from_web_page || matches!(path, "newtab" | "welcome" | "known-issues" | "http-warning" | "compat" | "danger-continue")
}

fn html_response(
    request: &Request,
    body: String,
) -> Pin<Box<dyn Future<Output = Response> + Send>> {
    let mut response = Response::new(
        request.current_url(),
        ResourceFetchTiming::new(request.timing_type()),
    );
    response
        .headers
        .typed_insert(ContentType::from(mime_guess::from_ext("html").first_or_octet_stream()));
    *response.body.lock() = ResponseBody::Done(body.into_bytes());
    Box::pin(std::future::ready(response))
}

fn json_response(
    request: &Request,
    body: String,
) -> Pin<Box<dyn Future<Output = Response> + Send>> {
    let mut response = Response::new(
        request.current_url(),
        ResourceFetchTiming::new(request.timing_type()),
    );
    response.headers.typed_insert(ContentType::json());
    *response.body.lock() = ResponseBody::Done(body.into_bytes());
    Box::pin(std::future::ready(response))
}
