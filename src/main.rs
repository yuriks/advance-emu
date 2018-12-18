#![feature(
    arbitrary_self_types,
    generator_trait,
    generators,
    pin,
    test
)]
#![allow(unused)]

extern crate byteorder;
extern crate num;
extern crate sdl2;
extern crate test;

#[macro_use]
mod util;
#[macro_use]
mod scheduler;

mod cpu;
mod memory;
mod ppu;
mod system;

use byteorder::ByteOrder;
use byteorder::NativeEndian;
use byteorder::LE;
use ppu::LcdControllerRegs;
use sdl2::event::Event;
use sdl2::keyboard::Scancode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::Texture;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::mem;

fn load_file(filename: &str, expected_size: usize) -> Result<Vec<u8>, Box<Error>> {
    let mut file = File::open(filename)?;
    let mut buf = Vec::new();
    let read_size = file.read_to_end(&mut buf)?;

    if read_size == expected_size {
        Ok(buf)
    } else {
        Err("size mismatch".into())
    }
}

fn copy_line(rgbx_pixels: &mut [u8], line: &[u16]) {
    assert_eq!(line.len(), 240);
    for i in 0..240 {
        // GBA colors are already in the BGR555 format the texture needs, so there's no conversion
        // needed.
        NativeEndian::write_u16(&mut rgbx_pixels[i * 2..], line[i]);
    }
}

fn draw_screen(texture: &mut Texture, regs: &LcdControllerRegs, vram: &[u8], pals: &[u16]) {
    texture
        .with_lock(None, |pixels: &mut [u8], stride| {
            for screen_y in 0..160 {
                let line_buf = ppu::render_lcd_line(screen_y as u16, regs, vram, pals);
                copy_line(&mut pixels[screen_y * stride..][..stride], &line_buf);
            }
        })
        .unwrap();
}

fn convert_to_u16_vec(src: &[u8]) -> Vec<u16> {
    let sizeof = mem::size_of::<u16>();
    assert_eq!(src.len() % sizeof, 0);

    let mut new_vec = vec![0u16; src.len() / sizeof];
    LE::read_u16_into(src.as_ref(), new_vec.as_mut());
    new_vec
}

const _BRIN_REGS: &[(u32, u16)] = &[
    (0x0400_0000, 0x0100),
    (0x0400_0008, 0x5E00),
    (0x0400_0010, 0x00C0),
    (0x0400_0012, 0x0040),
];

const _PRIO_REGS: &[(u32, u16)] = &[
    (0x0400_0000, 0x1F40),
    (0x0400_0004, 0x0009),
    (0x0400_0008, 0x1C08),
    (0x0400_000A, 0x0584),
    (0x0400_000C, 0x0685),
    (0x0400_000E, 0x0786),
];

const BM_MODES_REGS: &[(u32, u16)] = &[
    (0x0400_0000, 0x0403),
    (0x0400_0004, 0x0002),
    (0x0400_000C, 0x0000),
];

fn main() -> Result<(), Box<Error>> {
    let sdl_context = sdl2::init()?;
    let sdl_video = sdl_context.video()?;

    let window = sdl_video.window("Advance", 240, 160).build()?;
    let mut canvas = window.into_canvas().build()?;

    let texture_creator = canvas.texture_creator();
    let mut lcd_texture =
        texture_creator.create_texture_streaming(PixelFormatEnum::BGR555, 240, 160)?;

    let mut lcd_regs = LcdControllerRegs::new();
    for &(addr, value) in BM_MODES_REGS.iter() {
        lcd_regs.write(addr, value as u32);
    }

    let pal_mem = convert_to_u16_vec(load_file("bm_modes-pal.bin", 1024)?.as_ref());
    let vram_mem = load_file("bm_modes-vram.bin", 96 * 1024)?;

    let mut event_loop = sdl_context.event_pump()?;
    'main_loop: loop {
        for event in event_loop.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main_loop,

                Event::KeyDown {
                    scancode: Some(scancode),
                    ..
                } => {
                    if scancode == Scancode::Escape {
                        break 'main_loop;
                    }
                    println!("Pressed {}", scancode);
                }
                Event::KeyUp { .. } => {}

                _ => (),
            }
        }

        draw_screen(
            &mut lcd_texture,
            &lcd_regs,
            vram_mem.as_ref(),
            pal_mem.as_ref(),
        );

        canvas.clear();
        canvas.copy(&lcd_texture, None, None)?;
        canvas.present();
    }

    Ok(())
}
