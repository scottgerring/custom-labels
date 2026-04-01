use custom_labels::process_context::{ProcessContext, ProcessContextWriter};
use custom_labels::process_context_ext::ProcessContextTlsExt;
use custom_labels::writer;
use custom_labels::KeyHandle;

use rand::Rng;
use tracing::info;

const KEY_HTTP_METHOD: KeyHandle = KeyHandle::new(0);
const KEY_HTTP_ROUTE: KeyHandle = KeyHandle::new(1);
const KEY_USER_ID: KeyHandle = KeyHandle::new(2);

static HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH"];
static HTTP_ROUTES: &[&str] = &[
    "/api/users",
    "/api/orders",
    "/api/products",
    "/api/auth/login",
    "/api/health",
];

fn rand_trace_id(rng: &mut impl Rng) -> [u8; 16] {
    let mut id = [0u8; 16];
    rng.fill(&mut id);
    id
}

fn rand_span_id(rng: &mut impl Rng) -> [u8; 8] {
    let mut id = [0u8; 8];
    rng.fill(&mut id);
    id
}

fn rand_user_id(rng: &mut impl Rng) -> String {
    format!("user-{}", rng.gen_range(1000u32..9999u32))
}

fn main() {
    tracing_subscriber::fmt::init();

    // Initialize with 640 bytes (recommended max for eBPF readers)
    writer::setup(640);

    // Publish process context so profilers can decode the key table
    let ctx = ProcessContext::new()
        .with_resource("service.name", "spin-example")
        .with_resource("service.version", "0.5.0")
        .with_tls_config([
            (KEY_HTTP_METHOD.0, "http.method"),
            (KEY_HTTP_ROUTE.0, "http.route"),
            (KEY_USER_ID.0, "user.id"),
        ]);

    let _writer = ProcessContextWriter::publish(&ctx)
        .expect("failed to publish process context");
    info!("Process context published");

    let mut rng = rand::thread_rng();

    loop {
        let trace_id = rand_trace_id(&mut rng);
        let span_id = rand_span_id(&mut rng);
        let method = HTTP_METHODS[rng.gen_range(0..HTTP_METHODS.len())];
        let route = HTTP_ROUTES[rng.gen_range(0..HTTP_ROUTES.len())];
        let user_id = rand_user_id(&mut rng);

        writer::with_trace_and_attrs(
            &trace_id,
            &span_id,
            [
                (KEY_HTTP_METHOD, method.as_bytes()),
                (KEY_HTTP_ROUTE, route.as_bytes()),
                (KEY_USER_ID, user_id.as_bytes()),
            ],
            || {
                // Simulate request work — spin for a moment so the profiler
                // has time to observe this record.
                for _ in 0..1_000_000 {
                    std::hint::black_box(0u64);
                }
            },
        );
    }
}
