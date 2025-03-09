use axum::{Router, middleware::map_request_with_state, response::Html, routing::get};
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() {
    // build our application with a route
    let app = Router::new().route("/", get(handler));

    let calling_myself = tokio::spawn(calling_myself());
    // run it
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    let web_server = axum::serve(listener, app);
    let _result = tokio::join!(calling_myself, web_server);
}

async fn handler() -> Html<&'static str> {
    Html("<h1>Hello, World!</h1>")
}

async fn calling_myself() {
    sleep(Duration::from_millis(5000)).await;
    let client = reqwest::Client::new();
    let _respone = client.get("http://127.0.0.1:3000/").send().await.unwrap();
    println!("called myself");
}
