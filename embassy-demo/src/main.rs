#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_stm32::eth::generic_smi::GenericSMI;
use embassy_stm32::eth::{Ethernet, PacketQueue};
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::peripherals::ETH;
use embassy_stm32::rng::Rng;
use embassy_stm32::{bind_interrupts, eth, peripherals, rng, Config};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use picoserve::{
    response::DebugValue,
    routing::{get, parse_path_segment},
};
use rand_core::RngCore;
use static_cell::make_static;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use picoserve::extract::State;

bind_interrupts!(struct Irqs {
    ETH => eth::InterruptHandler;
    RNG => rng::InterruptHandler<peripherals::RNG>;
});

type EthDevice = Ethernet<'static, ETH, GenericSMI>;

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<EthDevice>) -> ! {
    stack.run().await
}

#[derive(Clone, Copy)]
struct SharedControl(&'static Mutex<CriticalSectionRawMutex, Output<'static, peripherals::PE1>>);

struct AppState {
    shared_control: SharedControl,
}

impl picoserve::extract::FromRef<AppState> for SharedControl {
    fn from_ref(state: &AppState) -> Self {
        state.shared_control
    }
}

type AppRouter = impl picoserve::routing::PathRouter<AppState>;

const WEB_TASK_POOL_SIZE: usize = 4;

#[embassy_executor::task(pool_size = WEB_TASK_POOL_SIZE)]
async fn web_task(
    id: usize,
    stack: &'static Stack<EthDevice>,
    app: &'static picoserve::Router<AppRouter, AppState>,
    config: &'static picoserve::Config<Duration>,
    state: AppState,
) -> ! {
    let port = 8080;
    let mut tcp_rx_buffer = [0; 1024];
    let mut tcp_tx_buffer = [0; 1024];
    let mut http_buffer = [0; 2048];

    picoserve::listen_and_serve_with_state(
        id,
        app,
        config,
        stack,
        port,
        &mut tcp_rx_buffer,
        &mut tcp_tx_buffer,
        &mut http_buffer,
        &state,
    )
    .await
}

#[embassy_executor::task]
async fn blinky_task(mut led: Output<'static, peripherals::PB0>) -> ! {
    loop {
        led.toggle();
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hsi = Some(HSIPrescaler::DIV1);
        config.rcc.csi = true;
        config.rcc.hsi48 = Some(Default::default()); // needed for RNG
        config.rcc.pll1 = Some(Pll {
            source: PllSource::HSI,
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL50,
            divp: Some(PllDiv::DIV2),
            divq: None,
            divr: None,
        });
        config.rcc.sys = Sysclk::PLL1_P; // 400 Mhz
        config.rcc.ahb_pre = AHBPrescaler::DIV2; // 200 Mhz
        config.rcc.apb1_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb2_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb3_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb4_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.voltage_scale = VoltageScale::Scale1;
    }
    let p = embassy_stm32::init(config);

    let led1 = Output::new(p.PB0, Level::High, Speed::Low); // green LED on Nucleo
    let led2 = Output::new(p.PE1, Level::High, Speed::Low); // yellow LED on Nucleo
    let led3 = Output::new(p.PB14, Level::High, Speed::Low); // red LED on Nucleo

    info!("Hello World!");

    unwrap!(spawner.spawn(blinky_task(led1)));

    // Generate random seed.
    let mut rng = Rng::new(p.RNG, Irqs);
    let mut seed = [0; 8];
    rng.fill_bytes(&mut seed);
    let seed = u64::from_le_bytes(seed);

    let mac_addr = [0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];

    static PACKETS: StaticCell<PacketQueue<4, 4>> = StaticCell::new();
    // warning: Not all STM32H7 devices have the exact same pins here
    // for STM32H747XIH, replace p.PB13 for PG12
    let device = Ethernet::new(
        PACKETS.init(PacketQueue::<4, 4>::new()),
        p.ETH,
        Irqs,
        p.PA1,  // ref_clk
        p.PA2,  // mdio
        p.PC1,  // eth_mdc
        p.PA7,  // CRS_DV: Carrier Sense
        p.PC4,  // RX_D0: Received Bit 0
        p.PC5,  // RX_D1: Received Bit 1
        p.PG13, // TX_D0: Transmit Bit 0
        p.PB13, // TX_D1: Transmit Bit 1
        p.PG11, // TX_EN: Transmit Enable
        GenericSMI::new(0),
        mac_addr,
    );

    let config = embassy_net::Config::dhcpv4(Default::default());

    // Init network stack
    static STACK: StaticCell<Stack<EthDevice>> = StaticCell::new();
    static RESOURCES: StaticCell<StackResources<WEB_TASK_POOL_SIZE>> = StaticCell::new();
    let stack = &*STACK.init(Stack::new(
        device,
        config,
        RESOURCES.init(StackResources::<WEB_TASK_POOL_SIZE>::new()),
        seed,
    ));

    // Launch network task
    unwrap!(spawner.spawn(net_task(&stack)));

    // Ensure DHCP configuration is up before trying connect
    stack.wait_config_up().await;

    info!("Network task initialized");

    fn make_app() -> picoserve::Router<AppRouter, AppState> {
        picoserve::Router::new()
            .route(
                "/",
                get(|| picoserve::response::File::html(include_str!("index.html"))),
            )
            .route(
                "/index.css",
                get(|| picoserve::response::File::css(include_str!("index.css"))),
            )
            .route(
                "/index.js",
                get(|| picoserve::response::File::javascript(include_str!("index.js"))),
            )
            .route(
                ("/toggle_led", parse_path_segment()),
                get(
                    |led_type: u8, State(SharedControl(control)): State<SharedControl>| async move {
                        info!("Toggling LED{}", led_type);
                        let mut control = control.lock().await;
                        control.toggle();
                        let led_state = control.is_set_high();
                        DebugValue(if led_state { "ON" } else { "OFF" })
                    },
                ),
            )
    }

    let app = make_static!(make_app());

    let config = make_static!(picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Some(Duration::from_secs(5)),
        read_request: Some(Duration::from_secs(1)),
        write: Some(Duration::from_secs(1)),
    })
    .keep_connection_alive());

    let shared_control = SharedControl(make_static!(Mutex::new(led2)));

    // for id in 0..WEB_TASK_POOL_SIZE {
    for id in 0..1 {
        spawner.must_spawn(web_task(
            id,
            stack,
            app,
            config,
            AppState { shared_control },
        ));
    }
}
