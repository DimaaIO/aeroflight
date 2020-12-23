#![no_main]
#![no_std]

#[macro_use]
extern crate cortex_m_semihosting;
extern crate panic_halt;

use aeroflight_components::{e32, internal, neo6m};
use stm32f4xx_hal::{gpio, i2c, pac, prelude::*, serial, stm32, timer};

type I2c1 = i2c::I2c<
    pac::I2C1,
    (
        gpio::gpiob::PB6<gpio::AlternateOD<gpio::AF4>>,
        gpio::gpiob::PB7<gpio::AlternateOD<gpio::AF4>>,
    ),
>;

type Serial1 = serial::Serial<
    pac::USART1,
    (
        gpio::gpioa::PA9<gpio::Alternate<gpio::AF7>>,
        gpio::gpioa::PA10<gpio::Alternate<gpio::AF7>>,
    ),
>;

type Serial2 = serial::Serial<
    pac::USART2,
    (
        gpio::gpioa::PA2<gpio::Alternate<gpio::AF7>>,
        gpio::gpioa::PA3<gpio::Alternate<gpio::AF7>>,
    ),
>;

type PcbLed = gpio::gpioc::PC13<gpio::Output<gpio::OpenDrain>>;

type ControllerAux = gpio::gpioa::PA8<gpio::Input<gpio::PullUp>>;
type ControllerM0 = gpio::gpiob::PB14<gpio::Output<gpio::OpenDrain>>;
type ControllerM1 = gpio::gpiob::PB15<gpio::Output<gpio::OpenDrain>>;
type Controller = e32::E32<pac::USART1>;

type GpsModule = neo6m::Neo6m<pac::USART2>;
type FlightControl = internal::FlightControl<pac::I2C1>;

#[rtic::app(device = stm32f4xx_hal::pac, peripherals = true, monotonic = rtic::cyccnt::CYCCNT)]
const APP: () = {
    struct Resources {
        i2c1: I2c1,
        serial1: Serial1,
        serial2: Serial2,

        controller_aux: ControllerAux,
        controller_m0: ControllerM0,
        controller_m1: ControllerM1,
        controller: Controller,
        gps: GpsModule,
        fc: FlightControl,
        tim2: timer::Timer<pac::TIM2>,
        pcb_led: PcbLed,
    }

    #[init]
    fn init(cx: init::Context) -> init::LateResources {
        let core: cortex_m::Peripherals = cx.core;
        let device: stm32::Peripherals = cx.device;

        let rcc = device.RCC.constrain();

        let clocks = rcc.cfgr.use_hse(25.mhz()).sysclk(48.mhz()).freeze();

        let gpioa = device.GPIOA.split();
        let gpiob = device.GPIOB.split();
        let gpioc = device.GPIOC.split();

        let i2c1 = i2c::I2c::i2c1(
            device.I2C1,
            (
                gpiob.pb6.into_alternate_af4_open_drain(),
                gpiob.pb7.into_alternate_af4_open_drain(),
            ),
            400.khz(),
            clocks,
        );

        let serial1 = serial::Serial::usart1(
            device.USART1,
            (
                gpioa.pa9.into_alternate_af7(),
                gpioa.pa10.into_alternate_af7(),
            ),
            serial::config::Config::default()
                .baudrate(9600.bps())
                .parity_even()
                .stopbits(serial::config::StopBits::STOP1)
                .wordlength_8(),
            clocks,
        )
        .unwrap();

        let serial2 = serial::Serial::usart2(
            device.USART2,
            (
                gpioa.pa2.into_alternate_af7(),
                gpioa.pa3.into_alternate_af7(),
            ),
            serial::config::Config::default()
                .baudrate(9600.bps())
                .parity_even()
                .stopbits(serial::config::StopBits::STOP1)
                .wordlength_8(),
            clocks,
        )
        .unwrap();

        let controller_aux = gpioa.pa8.into_pull_up_input();
        let controller_m0 = gpiob.pb14.into_open_drain_output();
        let controller_m1 = gpiob.pb15.into_open_drain_output();
        let controller = e32::E32::<pac::USART1>::new();

        let gps = neo6m::Neo6m::<pac::USART2>::new();
        let fc = internal::FlightControl::<pac::I2C1>::new();

        let mut tim2 = timer::Timer::tim2(device.TIM2, 1.hz(), clocks);
        tim2.listen(timer::Event::TimeOut);

        init::LateResources {
            i2c1,
            serial1,
            serial2,

            controller_aux,
            controller_m0,
            controller_m1,
            controller,

            gps,
            fc,
            tim2,
            pcb_led: gpioc.pc13.into_open_drain_output(),
        }
    }

    #[idle]
    fn idle(cx: idle::Context) -> ! {
        loop {
            cortex_m::asm::wfi();
        }
    }

    #[task(binds = TIM2, resources = [tim2, i2c1, fc, pcb_led])]
    fn tim2(cx: tim2::Context) {
        let tim2: &mut timer::Timer<pac::TIM2> = cx.resources.tim2;
        let pcb_led: &mut PcbLed = cx.resources.pcb_led;

        tim2.clear_interrupt(timer::Event::TimeOut);
        pcb_led.toggle().ok();
    }

    #[task(binds = USART1, resources = [serial1, controller, i2c1, fc])]
    fn usart1(cx: usart1::Context) {
        let serial1: &mut Serial1 = cx.resources.serial1;
        let controller: &mut Controller = cx.resources.controller;
        let i2c1: &mut I2c1 = cx.resources.i2c1;
        let fc: &mut FlightControl = cx.resources.fc;

        while serial1.is_rxne() {
            match controller.read(serial1) {
                Ok(msg) => {
                    match msg {
                        Some(msg) => {
                            use aeroflight_components::e32::command::Command;

                            match msg {
                                Command::Controller { right_trigger, .. } => {
                                    let throttle = right_trigger as u16 * 256;

                                    match fc.send(
                                        i2c1,
                                        internal::Command::ThrottleBulk {
                                            values: [Some(throttle); 4],
                                        },
                                    ) {
                                        Ok(_) => {}
                                        Err(_) => {}
                                    }
                                }
                            }
                        }
                        None => {}
                    }

                    break;
                }
                Err(_) => {}
            }
        }
    }

    #[task(binds = USART2, resources = [serial2, gps])]
    fn usart2(cx: usart2::Context) {
        let serial2: &mut Serial2 = cx.resources.serial2;
        let gps: &mut GpsModule = cx.resources.gps;

        if let Some(v) = gps.read(serial2) {}
    }
};
