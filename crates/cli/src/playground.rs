use std::{
    borrow::Cow,
    env, fs,
    net::TcpListener,
    path::{Path, PathBuf},
    str::FromStr as _,
};

use anyhow::{Context, Result, anyhow};
use log::{error, info};
use tiny_http::{Header, Response, Server};

use super::wasm;

const WEB_RUNTIME_URL: &str = "https://cdn.jsdelivr.net/npm/@willbooster/web-tree-sitter@20.0.0";

macro_rules! optional_resource {
    ($name:tt, $path:tt) => {
        #[cfg(TREE_SITTER_EMBED_WASM_BINDING)]
        fn $name(tree_sitter_dir: Option<&Path>) -> Cow<'static, [u8]> {
            if let Some(tree_sitter_dir) = tree_sitter_dir {
                Cow::Owned(fs::read(tree_sitter_dir.join($path)).unwrap())
            } else {
                Cow::Borrowed(include_bytes!(concat!("../../../", $path)))
            }
        }

        #[cfg(not(TREE_SITTER_EMBED_WASM_BINDING))]
        fn $name(tree_sitter_dir: Option<&Path>) -> Cow<'static, [u8]> {
            if let Some(tree_sitter_dir) = tree_sitter_dir {
                Cow::Owned(fs::read(tree_sitter_dir.join($path)).unwrap())
            } else {
                Cow::Borrowed(&[])
            }
        }
    };
}

optional_resource!(get_lib_js, "lib/binding_web/web-tree-sitter.js");
optional_resource!(get_lib_wasm, "lib/binding_web/web-tree-sitter.wasm");

struct PlaygroundAssets {
    main_html: String,
    playground_js: Cow<'static, [u8]>,
    lib_js: Cow<'static, [u8]>,
    lib_wasm: Cow<'static, [u8]>,
}

pub fn export(grammar_path: &Path, export_path: &Path) -> Result<()> {
    let (grammar_name, language_wasm) = wasm::load_language_wasm_file(grammar_path)?;

    fs::create_dir_all(export_path).with_context(|| {
        format!(
            "Failed to create export directory: {}",
            export_path.display()
        )
    })?;

    let mut assets = load_playground_assets(&grammar_name);
    let has_local_lib_js = !assets.lib_js.is_empty();
    let has_local_lib_wasm = !assets.lib_wasm.is_empty();

    if !has_local_lib_js {
        assets.main_html = assets.main_html.replace(
            "import * as TreeSitter from './web-tree-sitter.js';",
            &format!("import * as TreeSitter from '{WEB_RUNTIME_URL}/web-tree-sitter.js';"),
        );
    }

    fs::write(export_path.join("index.html"), assets.main_html.as_bytes())
        .with_context(|| "Failed to write index.html")?;

    fs::write(export_path.join("tree-sitter-parser.wasm"), language_wasm)
        .with_context(|| "Failed to write parser wasm file")?;

    fs::write(export_path.join("playground.js"), assets.playground_js)
        .with_context(|| "Failed to write playground.js")?;

    if has_local_lib_js {
        fs::write(export_path.join("web-tree-sitter.js"), assets.lib_js)
            .with_context(|| "Failed to write web-tree-sitter.js")?;
    }

    if has_local_lib_wasm {
        fs::write(export_path.join("web-tree-sitter.wasm"), assets.lib_wasm)
            .with_context(|| "Failed to write web-tree-sitter.wasm")?;
    }

    println!(
        "Exported playground to {}",
        export_path.canonicalize()?.display()
    );

    Ok(())
}

pub fn serve(grammar_path: &Path, open_in_browser: bool) -> Result<()> {
    let server = get_server()?;
    let (grammar_name, language_wasm) = wasm::load_language_wasm_file(grammar_path)?;
    let url = format!("http://{}", server.server_addr());
    info!("Started playground on: {url}");
    if open_in_browser && webbrowser::open(&url).is_err() {
        error!("Failed to open '{url}' in a web browser");
    }

    let assets = load_playground_assets(&grammar_name);
    let main_html = assets.main_html.into_bytes();
    let html_header = Header::from_str("Content-Type: text/html").unwrap();
    let js_header = Header::from_str("Content-Type: application/javascript").unwrap();
    let wasm_header = Header::from_str("Content-Type: application/wasm").unwrap();

    for request in server.incoming_requests() {
        let res = match request.url() {
            "/" => response(&main_html, &html_header),
            "/tree-sitter-parser.wasm" => response(&language_wasm, &wasm_header),
            "/playground.js" => response(&assets.playground_js, &js_header),
            "/web-tree-sitter.js" => asset_or_redirect(
                &assets.lib_js,
                &js_header,
                &format!("{WEB_RUNTIME_URL}/web-tree-sitter.js"),
            ),
            "/web-tree-sitter.wasm" => asset_or_redirect(
                &assets.lib_wasm,
                &wasm_header,
                &format!("{WEB_RUNTIME_URL}/web-tree-sitter.wasm"),
            ),
            _ => response(b"Not found", &html_header).with_status_code(404),
        };
        request
            .respond(res)
            .with_context(|| "Failed to write HTTP response")?;
    }

    Ok(())
}

fn load_playground_assets(grammar_name: &str) -> PlaygroundAssets {
    let tree_sitter_dir = env::var("TREE_SITTER_BASE_DIR").map(PathBuf::from).ok();
    let dir = tree_sitter_dir.as_deref();
    let raw_html: Cow<'_, str> = dir
        .map_or(Cow::Borrowed(include_str!("playground.html")), |dir| {
            Cow::Owned(fs::read_to_string(dir.join("crates/cli/src/playground.html")).unwrap())
        });
    PlaygroundAssets {
        main_html: raw_html.replace("THE_LANGUAGE_NAME", grammar_name),
        playground_js: dir.map_or(Cow::Borrowed(include_bytes!("playground.js")), |dir| {
            Cow::Owned(fs::read(dir.join("crates/cli/src/playground.js")).unwrap())
        }),
        lib_js: get_lib_js(dir),
        lib_wasm: get_lib_wasm(dir),
    }
}

fn asset_or_redirect<'a>(
    asset: &'a [u8],
    header: &Header,
    fallback_url: &str,
) -> Response<&'a [u8]> {
    if asset.is_empty() {
        redirect(fallback_url)
    } else {
        response(asset, header)
    }
}

fn redirect(url: &str) -> Response<&'static [u8]> {
    Response::empty(302)
        .with_data(b"".as_slice(), Some(0))
        .with_header(Header::from_bytes("Location", url.as_bytes()).unwrap())
}

fn response<'a>(data: &'a [u8], header: &Header) -> Response<&'a [u8]> {
    Response::empty(200)
        .with_data(data, Some(data.len()))
        .with_header(header.clone())
}

fn get_server() -> Result<Server> {
    let addr = env::var("TREE_SITTER_PLAYGROUND_ADDR").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = env::var("TREE_SITTER_PLAYGROUND_PORT")
        .map(|v| {
            v.parse::<u16>()
                .with_context(|| "Invalid port specification")
        })
        .ok();
    let listener = match port {
        Some(port) => {
            bind_to(&addr, port?).with_context(|| "Failed to bind to the specified port")?
        }
        None => get_listener_on_available_port(&addr)
            .with_context(|| "Failed to find a free port to bind to it")?,
    };
    let server =
        Server::from_listener(listener, None).map_err(|_| anyhow!("Failed to start web server"))?;
    Ok(server)
}

fn get_listener_on_available_port(addr: &str) -> Option<TcpListener> {
    (8000..12000).find_map(|port| bind_to(addr, port))
}

fn bind_to(addr: &str, port: u16) -> Option<TcpListener> {
    TcpListener::bind(format!("{addr}:{port}")).ok()
}
