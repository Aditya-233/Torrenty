use reqwest::Client;

#[tokio::test]
async fn test_reqwest_direct() {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
        .proxy(reqwest::Proxy::all("http://127.0.0.1:8080").unwrap())
        .build()
        .unwrap();

    println!("Testing reqwest through http://127.0.0.1:8080 ...");
    match client.get("https://nyaa.si/?page=rss&q=SubsPlease+Slime+20+1080p&c=0_0&f=0").send().await {
        Ok(resp) => {
            println!("Success via 8080: {}", resp.status());
            let text = resp.text().await.unwrap();
            println!("Response len: {}", text.len());
        },
        Err(e) => println!("Error via 8080: {:?}", e),
    }
}

#[tokio::test]
async fn test_tracker_announce() {
    let url = "http://nyaa.tracker.wf:7777/announce?info_hash=%1a%17%62%46%b6%ab%e4%f3%91%2c%f5%9e%5d%4c%f7%e8%f4%f7%08%17&peer_id=-rQ8110-123456789012&event=started&port=42400&uploaded=0&downloaded=0&left=1000000&compact=1";

    let default_client = Client::builder().build().unwrap();
    println!("Testing default client (librqbit style with socks_proxy_url=None)...");
    match default_client.get(url).send().await {
        Ok(resp) => println!("Default client status: {}", resp.status()),
        Err(e) => println!("Default client error: {:?}", e),
    }

    let proxy_client = Client::builder().proxy(reqwest::Proxy::all("http://127.0.0.1:8080").unwrap()).build().unwrap();
    println!("Testing proxy client (explicit http://127.0.0.1:8080)...");
    match proxy_client.get(url).send().await {
        Ok(resp) => {
            println!("Proxy client status: {}", resp.status());
            let b = resp.bytes().await.unwrap();
            println!("Proxy client response len: {}", b.len());
        },
        Err(e) => println!("Proxy client error: {:?}", e),
    }
}
