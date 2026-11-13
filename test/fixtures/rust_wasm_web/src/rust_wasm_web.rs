use std::{
    alloc::{Layout, alloc_zeroed},
    cell::UnsafeCell,
    future::Future,
    marker::PhantomPinned,
    pin::Pin,
    slice, str,
    task::{Context, Poll, Waker},
};
use tree_sitter::{Language, LanguageError, Parser, Tree, ffi::TSLanguage};

type Application = Pin<Box<dyn Future<Output = ()>>>;

const JAVASCRIPT: u32 = 0;
const PYTHON: u32 = 1;
const RUBY: u32 = 2;

struct ApplicationSlot(UnsafeCell<Option<Application>>);

unsafe impl Sync for ApplicationSlot {}

static APPLICATION: ApplicationSlot = ApplicationSlot(UnsafeCell::new(None));

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn request_parse(
        language_id: u32,
        source_address: u32,
        source_length: u32,
        request_address: u32,
    );
    fn log(message_address: u32, message_length: u32);
    fn notify_parsing_started();
}

fn log_progress(message: &str) {
    unsafe { log(message.as_ptr() as u32, message.len() as u32) }
}

struct ParseRequest<'a> {
    language_id: u32,
    source: &'a str,
    requested: bool,
    result: Option<Tree>,
    _pinned: PhantomPinned,
}

impl Future for ParseRequest<'_> {
    type Output = Tree;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if let Some(tree) = this.result.take() {
            return Poll::Ready(tree);
        }
        if !this.requested {
            this.requested = true;
            let request_address = std::ptr::from_mut(this) as u32;
            unsafe {
                request_parse(
                    this.language_id,
                    this.source.as_ptr() as u32,
                    this.source.len() as u32,
                    request_address,
                );
            }
        }
        Poll::Pending
    }
}

fn spawn_parse(language_id: u32, source: &str) -> ParseRequest<'_> {
    ParseRequest {
        language_id,
        source,
        requested: false,
        result: None,
        _pinned: PhantomPinned,
    }
}

async fn run() {
    let source = "const value = [0, 1, 2]\n";
    let tree = spawn_parse(JAVASCRIPT, source).await;
    let language = tree.language();
    assert_eq!(language.name(), Some("javascript"));
    assert!(!language.is_parseable());
    assert!(!tree.root_node().language().is_parseable());
    assert_eq!(
        Parser::new().set_language(&language),
        Err(LanguageError::NotParseable)
    );
    assert_eq!(
        tree.root_node().to_sexp(),
        "(program (lexical_declaration (variable_declarator name: (identifier) value: (array (number) (number) (number)))))"
    );
    log_progress("parsed javascript");

    let python_source = "def answer():\n  return 42\n";
    let python_tree = spawn_parse(PYTHON, python_source).await;
    assert_eq!(
        python_tree.root_node().to_sexp(),
        "(module (function_definition name: (identifier) parameters: (parameters) body: (block (return_statement (integer)))))"
    );
    log_progress("parsed python");

    // Ruby's scanner uses wide-character classification for suffixed constants
    // and unquoted heredoc delimiters.
    let ruby_source = "Éclair!\nvalue = <<~ÉTÉ\n  héllo\nÉTÉ\n";
    let ruby_tree = spawn_parse(RUBY, ruby_source).await;
    assert_eq!(
        ruby_tree.root_node().to_sexp(),
        "(program (call method: (constant)) (assignment left: (identifier) right: (heredoc_beginning)) (heredoc_body (heredoc_content) (heredoc_end)))"
    );
    log_progress("parsed ruby");
}

fn poll_application() -> bool {
    let application = unsafe { &mut *APPLICATION.0.get() };
    let future = application.as_mut().unwrap();
    let mut context = Context::from_waker(Waker::noop());
    if future.as_mut().poll(&mut context).is_ready() {
        *application = None;
        true
    } else {
        false
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn allocate_language_memory(size: u32, alignment: u32) -> u32 {
    let layout = Layout::from_size_align(size as usize, alignment as usize).unwrap();
    unsafe { alloc_zeroed(layout) as u32 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn run_parse(
    language_address: u32,
    source_address: u32,
    source_length: u32,
) -> u32 {
    let tree = run_parse_internal(language_address, source_address, source_length).ok();
    tree.map_or(0, |tree| tree.into_raw() as u32)
}

fn run_parse_internal(
    language_address: u32,
    source_address: u32,
    source_length: u32,
) -> Result<Tree, u32> {
    let source_bytes =
        unsafe { slice::from_raw_parts(source_address as *const u8, source_length as usize) };
    let Ok(source) = str::from_utf8(source_bytes) else {
        return Err(1);
    };

    let mut parser = Parser::new();
    let language = if language_address == JAVASCRIPT {
        Language::from(tree_sitter_javascript::LANGUAGE)
    } else {
        unsafe { Language::from_raw(language_address as *const TSLanguage) }
    };
    if parser.set_language(&language).is_err() {
        return Err(3);
    }
    let mut paused = false;
    let Some(tree) = parser.parse_with_options(
        &mut |byte_offset, _| {
            if !paused {
                paused = true;
                unsafe { notify_parsing_started() };
            }
            source.as_bytes().get(byte_offset..).unwrap_or_default()
        },
        None,
    ) else {
        return Err(4);
    };
    Ok(tree)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn start() {
    let application = unsafe { &mut *APPLICATION.0.get() };
    assert!(application.is_none());
    *application = Some(Box::pin(run()));
    assert!(!poll_application());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tree_ready(request_address: u32, tree_address: u32) -> u32 {
    assert_ne!(request_address, 0);
    assert_ne!(tree_address, 0);
    let request = unsafe { &mut *(request_address as *mut ParseRequest<'static>) };
    assert!(request.requested);
    assert!(request.result.is_none());
    request.result = Some(unsafe { Tree::from_raw(tree_address as *mut _) });
    u32::from(poll_application())
}
