use iced::widget::{column, container, row, slider, text};
use iced::{Antialiasing, Element, Length};

use std::time::Duration;

mod circular;
mod easing;
mod linear;

use circular::Circular;
use linear::Linear;

pub fn main() -> iced::Result {
    iced::program(
        "Loading Spinners - Iced",
        LoadingSpinners::update,
        LoadingSpinners::view,
    )
    .antialiasing(Antialiasing::Auto)
    .run()
}

struct LoadingSpinners {
    cycle_duration: f32,
}

#[derive(Debug, Clone, Copy)]
enum Message {
    CycleDurationChanged(f32),
}

impl LoadingSpinners {
    fn update(&mut self, message: Message) {
        match message {
            Message::CycleDurationChanged(duration) => {
                self.cycle_duration = duration;
            }
        }
    }

    fn view(&self) -> Element<Message> {
        let column = [
            &easing::EMPHASIZED,
            &easing::EMPHASIZED_DECELERATE,
            &easing::EMPHASIZED_ACCELERATE,
            &easing::STANDARD,
            &easing::STANDARD_DECELERATE,
            &easing::STANDARD_ACCELERATE,
        ]
        .iter()
        .zip([
            "Emphasized:",
            "Emphasized Decelerate:",
            "Emphasized Accelerate:",
            "Standard:",
            "Standard Decelerate:",
            "Standard Accelerate:",
        ])
        .fold(column![], |column, (easing, label)| {
            column.push(
                row![
                    text(label).width(250),
                    Linear::new().easing(easing).cycle_duration(
                        Duration::from_secs_f32(self.cycle_duration)
                    ),
                    Circular::new().easing(easing).cycle_duration(
                        Duration::from_secs_f32(self.cycle_duration)
                    )
                ]
                .align_items(iced::Alignment::Center)
                .spacing(20.0),
            )
        })
        .spacing(20);

        container(
            column.push(
                row![
                    text("Cycle duration:"),
                    slider(1.0..=1000.0, self.cycle_duration * 100.0, |x| {
                        Message::CycleDurationChanged(x / 100.0)
                    })
                    .width(200.0),
                    text(format!("{:.2}s", self.cycle_duration)),
                ]
                .align_items(iced::Alignment::Center)
                .spacing(20.0),
            ),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x()
        .center_y()
        .into()
    }
}

impl Default for LoadingSpinners {
    fn default() -> Self {
        Self {
            cycle_duration: 2.0,
        }
    }
}
