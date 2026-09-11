//! Wi-Fi station with DHCP, reconnects and a public HTTP bring-up test.
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{Runner, Stack, StackResources, dns::DnsQueryType, tcp::TcpSocket};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::{peripherals::WIFI, rng::Rng};
use esp_println::println;
use esp_radio::wifi::{
    AuthenticationMethod, Config, ControllerConfig, Interface, WifiController, scan::ScanConfig,
    sta::StationConfig,
};
use static_cell::StaticCell;

mod config {
    include!(env!("CYCLING_WIFI_CONFIG"));
}

static STATE: AtomicU8 = AtomicU8::new(0);
static RECONNECT: AtomicBool = AtomicBool::new(false);
static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

pub fn label() -> &'static [u8] {
    match STATE.load(Ordering::Relaxed) {
        1 => b"WIFI CONNECTING",
        2 => b"WIFI GETTING IP",
        3 => b"WIFI CONNECTED",
        4 => b"WIFI TEST OK",
        5 => b"WIFI RETRYING",
        _ => b"WIFI NOT SET UP",
    }
}

#[embassy_executor::task]
pub async fn start(peripheral: WIFI<'static>, spawner: Spawner) {
    if config::SSID.is_empty() {
        println!("CYCLING_WIFI unconfigured");
        return;
    }
    STATE.store(1, Ordering::Relaxed);
    let auth = if config::WPA3 {
        AuthenticationMethod::Wpa3Personal
    } else {
        AuthenticationMethod::Wpa2Personal
    };
    let station = StationConfig::default()
        .with_ssid(config::SSID)
        .with_password(config::PASSWORD.into())
        .with_auth_method(auth);
    let mut controller = match WifiController::new(
        peripheral,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    ) {
        Ok(controller) => controller,
        Err(error) => {
            STATE.store(5, Ordering::Relaxed);
            println!("CYCLING_WIFI init_failed error={:?}", error);
            return;
        }
    };
    println!(
        "CYCLING_WIFI radio_ready heap_free={}",
        esp_alloc::HEAP.free()
    );
    match with_timeout(
        Duration::from_secs(15),
        controller.scan_async(&ScanConfig::default().with_ssid(config::SSID).with_max(8)),
    )
    .await
    {
        Ok(Ok(aps)) => println!("CYCLING_WIFI scan configured_network_matches={}", aps.len()),
        _ => println!("CYCLING_WIFI scan_failed"),
    }
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(network(runner).unwrap());
    spawner.spawn(verify(stack).unwrap());
}

async fn reconnect_requested() {
    loop {
        if RECONNECT.swap(false, Ordering::Relaxed) {
            return;
        }
        Timer::after_millis(250).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        STATE.store(1, Ordering::Relaxed);
        println!("CYCLING_WIFI connecting");
        match controller.connect_async().await {
            Ok(_) => {
                STATE.store(2, Ordering::Relaxed);
                println!("CYCLING_WIFI associated");
                match select(
                    controller.wait_for_disconnect_async(),
                    reconnect_requested(),
                )
                .await
                {
                    Either::First(_) => println!("CYCLING_WIFI disconnected"),
                    Either::Second(_) => {
                        println!("CYCLING_WIFI reconnect_test disconnecting");
                        if let Err(error) = controller.disconnect_async().await {
                            println!("CYCLING_WIFI disconnect_failed error={:?}", error);
                        }
                    }
                }
            }
            Err(error) => println!("CYCLING_WIFI connect_failed error={:?}", error),
        }
        STATE.store(5, Ordering::Relaxed);
        Timer::after_secs(5).await;
    }
}

#[embassy_executor::task]
async fn network(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

#[embassy_executor::task]
async fn verify(stack: Stack<'static>) {
    let mut successes = 0u32;
    let mut reconnect_tested = false;
    loop {
        if with_timeout(Duration::from_secs(30), stack.wait_config_up())
            .await
            .is_err()
        {
            println!("CYCLING_WIFI dhcp_waiting");
            continue;
        }
        STATE.store(3, Ordering::Relaxed);
        // Network addresses are intentionally omitted from logs and the public UI.
        println!("CYCLING_WIFI dhcp_ready");
        let dns_ok = matches!(with_timeout(Duration::from_secs(10), stack.dns_query("example.com", DnsQueryType::A)).await, Ok(Ok(addresses)) if !addresses.is_empty());
        println!("CYCLING_WIFI dns_ok={}", dns_ok);
        {
            match with_timeout(Duration::from_secs(15), probe(stack)).await {
                Ok(Ok(())) => {
                    successes = successes.saturating_add(1);
                    STATE.store(4, Ordering::Relaxed);
                    println!(
                        "CYCLING_WIFI http_verified count={} heap_free={}",
                        successes,
                        esp_alloc::HEAP.free()
                    );
                    if reconnect_tested {
                        stack.wait_config_down().await;
                        continue;
                    }
                    if !reconnect_tested {
                        // Repeat the public HTTP check after a deliberate reconnect.
                        reconnect_tested = true;
                        Timer::after_secs(3).await;
                        RECONNECT.store(true, Ordering::Relaxed);
                        let _ =
                            with_timeout(Duration::from_secs(10), stack.wait_config_down()).await;
                        continue;
                    }
                }
                _ => println!("CYCLING_WIFI http_test_failed"),
            }
        }
        let _ = with_timeout(Duration::from_secs(30), stack.wait_config_down()).await;
    }
}

async fn probe(stack: Stack<'static>) -> Result<(), ()> {
    let mut rx = [0; 1024];
    let mut tx = [0; 512];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(Duration::from_secs(10)));
    let addresses = stack
        .dns_query("example.com", DnsQueryType::A)
        .await
        .map_err(|_| ())?;
    let host = *addresses.first().ok_or(())?;
    socket.connect((host, 80)).await.map_err(|_| ())?;
    let mut request =
        b"GET / HTTP/1.0\r\nHost: example.com\r\nConnection: close\r\n\r\n".as_slice();
    while !request.is_empty() {
        let n = socket.write(request).await.map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        request = &request[n..];
    }
    let mut response = [0u8; 4096];
    let mut used = 0;
    loop {
        if used == response.len() {
            return Err(());
        }
        let n = socket.read(&mut response[used..]).await.map_err(|_| ())?;
        if n == 0 {
            break;
        }
        used += n;
    }
    let response = &response[..used];
    if cycling_os::network::verified_response(response) {
        Ok(())
    } else {
        Err(())
    }
}
