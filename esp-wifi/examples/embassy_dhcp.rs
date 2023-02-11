#![no_std]
#![no_main]
#![feature(c_variadic)]
#![feature(const_mut_refs)]
#![feature(type_alias_impl_trait)]

use embassy_executor::_export::StaticCell;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config, Ipv4Address, Stack, StackResources};
#[cfg(feature = "esp32")]
use esp32_hal as hal;
#[cfg(feature = "esp32c2")]
use esp32c2_hal as hal;
#[cfg(feature = "esp32c3")]
use esp32c3_hal as hal;
#[cfg(feature = "esp32s2")]
use esp32s2_hal as hal;
#[cfg(feature = "esp32s3")]
use esp32s3_hal as hal;

use embassy_executor::Executor;
use embassy_time::{Duration, Timer};
use embedded_svc::wifi::{AccessPointInfo, ClientConfiguration, Configuration, Wifi};
use esp_backtrace as _;
use esp_println::logger::init_logger;
use esp_println::println;
use esp_wifi::initialize;
use esp_wifi::wifi::{WifiDevice, WifiError, WifiState, WifiEvent};
use hal::clock::{ClockControl, CpuClock};
use hal::Rng;
use hal::{embassy, peripherals::Peripherals, prelude::*, timer::TimerGroup, Rtc};

#[cfg(any(feature = "esp32c3", feature = "esp32c2"))]
use hal::system::SystemExt;

#[cfg(any(feature = "esp32c3", feature = "esp32c2"))]
use riscv_rt::entry;
#[cfg(any(feature = "esp32", feature = "esp32s3", feature = "esp32s2"))]
use xtensa_lx_rt::entry;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

macro_rules! singleton {
    ($val:expr) => {{
        type T = impl Sized;
        static STATIC_CELL: StaticCell<T> = StaticCell::new();
        let (x,) = STATIC_CELL.init(($val,));
        x
    }};
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[entry]
fn main() -> ! {
    init_logger(log::LevelFilter::Info);
    esp_wifi::init_heap();

    let peripherals = Peripherals::take();

    #[cfg(not(feature = "esp32"))]
    let system = peripherals.SYSTEM.split();
    #[cfg(feature = "esp32")]
    let system = peripherals.DPORT.split();

    #[cfg(feature = "esp32c3")]
    let clocks = ClockControl::configure(system.clock_control, CpuClock::Clock160MHz).freeze();
    #[cfg(feature = "esp32c2")]
    let clocks = ClockControl::configure(system.clock_control, CpuClock::Clock120MHz).freeze();
    #[cfg(any(feature = "esp32", feature = "esp32s3", feature = "esp32s2"))]
    let clocks = ClockControl::configure(system.clock_control, CpuClock::Clock240MHz).freeze();

    let mut rtc = Rtc::new(peripherals.RTC_CNTL);

    // Disable watchdog timers
    #[cfg(not(any(feature = "esp32", feature = "esp32s2")))]
    rtc.swd.disable();

    rtc.rwdt.disable();

    #[cfg(any(feature = "esp32c3", feature = "esp32c2"))]
    {
        use hal::systimer::SystemTimer;
        let syst = SystemTimer::new(peripherals.SYSTIMER);
        initialize(syst.alarm0, Rng::new(peripherals.RNG), &clocks).unwrap();
    }
    #[cfg(any(feature = "esp32", feature = "esp32s3", feature = "esp32s2"))]
    {
        use hal::timer::TimerGroup;
        let timg1 = TimerGroup::new(peripherals.TIMG1, &clocks);
        initialize(timg1.timer0, Rng::new(peripherals.RNG), &clocks).unwrap();
    }

    let wifi_interface = WifiDevice::new();

    let timer_group0 = TimerGroup::new(peripherals.TIMG0, &clocks);
    embassy::init(&clocks, timer_group0.timer0);

    let config = Config::Dhcp(Default::default());

    let seed = 1234; // very random, very secure seed

    // Init network stack
    let stack = &*singleton!(Stack::new(
        wifi_interface,
        config,
        singleton!(StackResources::<3>::new()),
        seed
    ));

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        // spawner.spawn(net_task(&stack)).ok();
        // spawner.spawn(task(&stack)).ok();
        // spawner.spawn(pinger()).ok();
        // spawner.spawn(scanner(wifi_interface)).ok();
        spawner.spawn(connection(&stack)).ok();
    });
}

#[embassy_executor::task]
async fn connection(_stack: &'static Stack<WifiDevice>) {
    println!("start connection task");
    let mut is_started = false; // this is a replacement for buggy device.is_started().unwrap() impl
    loop {
        let mut device = WifiDevice::new(); // TODO THIS IS BAD - but there is no way to get access to the device in the stack at the moment :()
        match esp_wifi::wifi::get_wifi_state() {
            WifiState::StaConnected => {
                // wait until we're no longer connected
                WifiEvent::StaDisconnected.await;
            },
            s => println!("In state: {s:?}, moving to connect")
        }
        if !is_started {
            println!("Starting wifi");
            device.start().await.unwrap();
            is_started = true;
            println!("Wifi started!");
        }
        let client_config = Configuration::Client(ClientConfiguration {
            ssid: SSID.into(),
            password: PASSWORD.into(),
            ..Default::default()
        });
        device.set_configuration(&client_config).unwrap();

        println!("{:?}", device.get_capabilities());
        println!("About to connect...");

        match device.connect().await {
            Ok(_) => println!("Wifi connected!"),
            Err(e) => println!("Failed to connect to wifi: {e:?}"),
        }

        Timer::after(Duration::from_millis(1000)).await
    }
}


// #[embassy_executor::task]
// async fn scanner(mut wifi_interface: WifiDevice) {
//     loop {
//         println!("Begin scan...");
//         let res: Result<(heapless::Vec<AccessPointInfo, 10>, usize), WifiError> =
//             wifi_interface.scan_n().await;
//         match res {
//             Ok((res, n)) => {
//                 println!("Scan returned {} results, ", n);
//                 for ap in res {
//                     println!("{:?}", ap);
//                 }
//             },
//             Err(e) => println!("Scan error: {:?}", e)
//         }
//         Timer::after(Duration::from_millis(3000)).await;
//     }
// }

#[embassy_executor::task]
async fn pinger() {
    loop {
        println!("Ping!");
        Timer::after(Duration::from_millis(1000)).await;
    }
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<WifiDevice>) {
    stack.run().await
}

#[embassy_executor::task]
async fn task(stack: &'static Stack<WifiDevice>) {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    loop {
        Timer::after(Duration::from_millis(1_000)).await;

        let mut socket = TcpSocket::new(&stack, &mut rx_buffer, &mut tx_buffer);

        socket.set_timeout(Some(embassy_net::SmolDuration::from_secs(10)));

        let remote_endpoint = (Ipv4Address::new(142, 250, 185, 115), 80);
        println!("connecting...");
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            println!("connect error: {:?}", e);
            continue;
        }
        println!("connected!");
        let mut buf = [0; 1024];
        loop {
            use embedded_io::asynch::Write;
            let r = socket
                .write_all(b"GET / HTTP/1.0\r\nHost: www.mobile-j.de\r\n\r\n")
                .await;
            if let Err(e) = r {
                println!("write error: {:?}", e);
                break;
            }
            let n = match socket.read(&mut buf).await {
                Ok(0) => {
                    println!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    println!("read error: {:?}", e);
                    break;
                }
            };
            println!("{}", core::str::from_utf8(&buf[..n]).unwrap());
        }
        Timer::after(Duration::from_millis(1000)).await;
    }
}
