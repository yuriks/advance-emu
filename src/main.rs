#![feature(macro_literal_matcher, min_const_fn)]

extern crate byteorder;
extern crate num;
extern crate sdl2;

use byteorder::ByteOrder;
use byteorder::NativeEndian;
use byteorder::LE;
use sdl2::event::Event;
use sdl2::keyboard::Scancode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::Texture;
use std::error::Error;
use std::fs::File;
use std::io::Read;

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

#[derive(Copy, Clone, Debug, PartialEq)]
enum BgColorMode {
    Pal16,
    Pal256,
}

const NUM_BG_LAYERS: usize = 4;

#[derive(Copy, Clone)]
struct BgAttributes {
    priority: u8,  // 0-3
    char_base: u8, // 0-3, units of 16 KB
    color_mode: BgColorMode,
    map_base: u8,  // 0-31, units of 2 KB
    size_mode: u8, // 0-3, see table in GBATEK
    x_scroll: u16, // 0-511
    y_scroll: u16, // 0-511
}

impl BgAttributes {
    const fn new() -> Self {
        BgAttributes {
            priority: 0,
            char_base: 0,
            color_mode: BgColorMode::Pal16,
            map_base: 0,
            size_mode: 0,
            x_scroll: 0,
            y_scroll: 0,
        }
    }
}

struct LcdControllerRegs {
    // DISPCNT
    video_mode: u8,
    active_display_page: u8,
    forced_blank_enabled: bool,
    bg_layer_enabled: [bool; NUM_BG_LAYERS],

    // BGxCNT
    bg_attributes: [BgAttributes; NUM_BG_LAYERS],
}

macro_rules! bit {
    ($data:ident[$base:literal : $limit:literal]) => (bit!($data[$base; $limit - $base + 1]));
    ($data:ident[$bit:expr]) => (($data >> $bit) & 1);
    ($data:ident[$base:expr; $len:expr]) => (($data >> $base) & (1 << $len) - 1);
}

impl LcdControllerRegs {
    const fn new() -> Self {
        LcdControllerRegs {
            video_mode: 0,
            active_display_page: 0,
            forced_blank_enabled: false,
            bg_layer_enabled: [false; NUM_BG_LAYERS],
            bg_attributes: [BgAttributes::new(); NUM_BG_LAYERS],
        }
    }

    fn write(&mut self, address: u32, data: u32) {
        match address & 0xFFF {
            0x000 => self.write_dispcnt(data as u16),
            0x008 => self.write_bgcnt(0, data as u16),
            0x00A => self.write_bgcnt(1, data as u16),
            0x00C => self.write_bgcnt(2, data as u16),
            0x00E => self.write_bgcnt(3, data as u16),
            0x010 => self.write_bghofs(0, data as u16),
            0x012 => self.write_bgvofs(0, data as u16),
            0x014 => self.write_bghofs(1, data as u16),
            0x016 => self.write_bgvofs(1, data as u16),
            0x018 => self.write_bghofs(2, data as u16),
            0x01A => self.write_bgvofs(2, data as u16),
            0x01C => self.write_bghofs(3, data as u16),
            0x01E => self.write_bgvofs(3, data as u16),
            _ => println!(
                "Unsupported LCD write: [0x{:08X}] <= 0x{:08X}",
                address, data
            ),
        }
    }

    fn write_dispcnt(&mut self, data: u16) {
        self.video_mode = bit!(data[0:2]) as u8;
        self.active_display_page = bit!(data[4]) as u8;
        self.forced_blank_enabled = bit!(data[7]) != 0;
        self.bg_layer_enabled[0] = bit!(data[8]) != 0;
        self.bg_layer_enabled[1] = bit!(data[9]) != 0;
        self.bg_layer_enabled[2] = bit!(data[10]) != 0;
        self.bg_layer_enabled[3] = bit!(data[11]) != 0;
    }

    fn write_bgcnt(&mut self, i: usize, data: u16) {
        let bg = &mut self.bg_attributes[i];
        bg.priority = bit!(data[0:1]) as u8;
        bg.char_base = bit!(data[2:3]) as u8;
        bg.color_mode = match bit!(data[7]) {
            0 => BgColorMode::Pal16,
            1 => BgColorMode::Pal256,
            _ => unreachable!(),
        };
        bg.map_base = bit!(data[8:12]) as u8;
        bg.size_mode = bit!(data[14:15]) as u8;
    }

    fn write_bghofs(&mut self, i: usize, data: u16) {
        self.bg_attributes[i].x_scroll = bit!(data[0:8]);
    }

    fn write_bgvofs(&mut self, i: usize, data: u16) {
        self.bg_attributes[i].y_scroll = bit!(data[0:8]);
    }
}

fn render_text_bg_line(
    screen_y: u16,
    bg: usize,
    regs: &LcdControllerRegs,
    vram: &[u8],
) -> [u8; 240] {
    let mut buf = [0; 240];

    let bg_regs = &regs.bg_attributes[bg];
    assert_eq!(bg_regs.color_mode, BgColorMode::Pal16);

    let character_base = bg_regs.char_base as usize * 0x4000;

    let bg_y = screen_y.wrapping_add(bg_regs.y_scroll);
    let map_y = bg_y % 256 / 8;
    let submap_y = bg_y / 256 % 2;
    let tile_y = (bg_y % 8) as usize;

    for screen_x in 0..240u16 {
        let bg_x = screen_x.wrapping_add(bg_regs.x_scroll);
        let map_x = bg_x % 256 / 8;
        let submap_x = bg_x / 256 % 2;
        let tile_x = (bg_x % 8) as usize;

        let submap = match bg_regs.size_mode {
            0 => 0,
            1 => submap_x,
            2 => submap_y,
            3 => submap_y * 2 + submap_x,
            _ => unreachable!(),
        } as usize;

        let map_base = (bg_regs.map_base as usize + submap) % 32 * 0x800;
        let map_offset = map_y as usize * (256 / 8) + map_x as usize;
        let entry = LE::read_u16(&vram[map_base + map_offset * 2..]);

        let tile_id = bit!(entry[0:9]) as usize;
        let h_flip = bit!(entry[10]) != 0;
        let v_flip = bit!(entry[11]) != 0;
        let pal_id = bit!(entry[12:15]);

        let flipped_tile_x = if h_flip { 7 - tile_x } else { tile_x };
        let flipped_tile_y = if v_flip { 7 - tile_y } else { tile_y };

        let mut pixel = vram[(tile_id * (8 * 8) + (flipped_tile_y * 8) + flipped_tile_x) / 2];
        if tile_x % 2 != 0 {
            pixel >>= 4;
        }
        pixel &= 0xF;

        buf[screen_x as usize] = pixel as u8;
    }

    buf
}

fn copy_line(rgbx_pixels: &mut [u8], line: &[u8], pal: &[u8]) {
    assert_eq!(line.len(), 240);
    for i in 0..240 {
        // GBA colors are already in the BGR555 format the texture needs, so we just need to convert
        // from LE to native (if required).
        let pal_entry = LE::read_u16(&pal[line[i] as usize * 2..]);
        NativeEndian::write_u16(&mut rgbx_pixels[i * 2..], pal_entry);
    }
}

fn draw_screen(texture: &mut Texture, regs: &LcdControllerRegs, vram: &[u8], pals: &[u8]) {
    texture
        .with_lock(None, |pixels: &mut [u8], stride| {
            for screen_y in 0..160 {
                let line_buf = render_text_bg_line(screen_y as u16, 0, regs, vram);
                let line_offset = screen_y * stride;
                copy_line(
                    &mut pixels[line_offset..line_offset + stride],
                    &line_buf,
                    &pals[32 * 3..32 * 4],
                );
            }
        })
        .unwrap();
}

fn main() -> Result<(), Box<Error>> {
    let sdl_context = sdl2::init()?;
    let sdl_video = sdl_context.video()?;

    let window = sdl_video.window("Advance", 240, 160).build()?;
    let mut canvas = window.into_canvas().build()?;

    let texture_creator = canvas.texture_creator();
    let mut lcd_texture =
        texture_creator.create_texture_streaming(PixelFormatEnum::BGR555, 240, 160)?;

    let mut lcd_regs = LcdControllerRegs::new();
    lcd_regs.write(0x0400_0000, 0x0100);
    lcd_regs.write(0x0400_0008, 0x5E00);
    lcd_regs.write(0x0400_0010, 0x00C0);
    lcd_regs.write(0x0400_0012, 0x0040);

    let mut bgx = 0xC0;
    let mut bgy = 0x40;

    let pal_mem = load_file("brin-pal.bin", 1024)?;
    let vram_mem = load_file("brin-vram.bin", 96 * 1024)?;

    assert_eq!(lcd_regs.video_mode, 0);

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
                    match scancode {
                        Scancode::A => bgx -= 8,
                        Scancode::D => bgx += 8,
                        Scancode::Left => bgx -= 1,
                        Scancode::Right => bgx += 1,

                        Scancode::W => bgy -= 8,
                        Scancode::S => bgy += 8,
                        Scancode::Up => bgy -= 1,
                        Scancode::Down => bgy += 1,

                        Scancode::Tab => {
                            bgx = 0;
                            bgy = 0;
                        }

                        _ => (),
                    }
                    lcd_regs.write(0x0400_0010, bgx as u32);
                    lcd_regs.write(0x0400_0012, bgy as u32);
                }
                Event::KeyUp { .. } => {}

                _ => (),
            }
        }

        draw_screen(
            &mut lcd_texture,
            &lcd_regs,
            &vram_mem[..64 * 1024],
            pal_mem.as_ref(),
        );

        canvas.clear();
        canvas.copy(&lcd_texture, None, None)?;
        canvas.present();
    }

    Ok(())
}
