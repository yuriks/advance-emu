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

#[derive(Copy, Clone, Debug, PartialEq)]
enum BgPaletteMode {
    Pal16,
    Pal256,
}

const NUM_BG_LAYERS: usize = 4;

#[derive(Copy, Clone)]
struct BgAttributes {
    priority: u8,  // 0-3
    char_base: u8, // 0-3, units of 16 KB
    palette_mode: BgPaletteMode,
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
            palette_mode: BgPaletteMode::Pal16,
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
        bg.palette_mode = match bit!(data[7]) {
            0 => BgPaletteMode::Pal16,
            1 => BgPaletteMode::Pal256,
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

fn render_text_bg_pixel(
    screen_y: u16,
    screen_x: u16,
    bg_id: u8,
    bg_regs: &BgAttributes,
    vram: &[u8],
    pals: &[u16],
) -> Option<Layer> {
    // Calculate tile and background coordinates
    fn calc_bg_coords(screen_y: u16, bg_y_scroll: u16) -> (usize, usize, usize) {
        let bg_y = screen_y.wrapping_add(bg_y_scroll) % 512;
        let tile_y = bg_y % 8;
        let map_y = bg_y / 8 % 32;
        let submap_y = bg_y / 8 / 32;
        (tile_y as usize, map_y as usize, submap_y as usize)
    }
    let (tile_x, map_x, submap_x) = calc_bg_coords(screen_x, bg_regs.x_scroll);
    let (tile_y, map_y, submap_y) = calc_bg_coords(screen_y, bg_regs.y_scroll);

    // Calculate map base/screenblock and offset
    let screenblock_offset = match bg_regs.size_mode {
        0 => 0,
        1 => submap_x,
        2 => submap_y,
        3 => submap_y * 2 + submap_x,
        _ => unreachable!(),
    };
    assert!(screenblock_offset < 4);

    let screenblock_base = (bg_regs.map_base as usize + screenblock_offset) * 0x800;
    let screenblock_offset = map_y * 32 + map_x;

    // Read map entry from VRAM
    let entry = LE::read_u16(&vram[screenblock_base + screenblock_offset * 2..]);
    let tile_id = bit!(entry[0:9]) as usize;
    let h_flip = bit!(entry[10]) != 0;
    let v_flip = bit!(entry[11]) != 0;
    let pal_id = bit!(entry[12:15]);

    // Calculate character data offset
    let flipped_tile_x = if h_flip { 7 - tile_x } else { tile_x };
    let flipped_tile_y = if v_flip { 7 - tile_y } else { tile_y };
    let charmap_base = bg_regs.char_base as usize * 0x4000;
    let charmap_offset = tile_id * (8 * 8) + (flipped_tile_y * 8) + flipped_tile_x;

    // Read pixel data and compute palette index
    let palette_index;
    let opaque;
    match bg_regs.palette_mode {
        BgPaletteMode::Pal16 => {
            let read_byte = vram[charmap_base + charmap_offset / 2];
            let pixel = read_byte >> (flipped_tile_x % 2 * 4) & 0xF;
            palette_index = pixel + (pal_id * 16) as u8;
            opaque = pixel != 0;
        }
        BgPaletteMode::Pal256 => {
            palette_index = vram[charmap_base + charmap_offset];
            opaque = palette_index != 0;
        }
    }

    // Read palette entry
    let color = pals[palette_index as usize];

    if opaque {
        Some(Layer {
            id: LayerId::Bg(bg_id),
            color,
            priority: bg_regs.priority,
            force_alpha_blend: false,
        })
    } else {
        None
    }
}

fn pick_top_two<T: Copy, K: Ord>(
    mut v: impl Iterator<Item = T>,
    key_fn: impl Fn(&T) -> K,
) -> (Option<T>, Option<T>) {
    if let Some(mut first) = v.next() {
        let mut second = None;
        for e in v {
            if key_fn(&e) < key_fn(&first) {
                second = Some(mem::replace(&mut first, e));
            }
        }
        (Some(first), second)
    } else {
        (None, None)
    }
}

#[derive(Copy, Clone)]
enum LayerId {
    _Obj,
    Bg(u8),
    Backdrop,
}

#[derive(Copy, Clone)]
struct Layer {
    #[allow(dead_code)]
    id: LayerId,
    color: u16,
    priority: u8,
    #[allow(dead_code)]
    force_alpha_blend: bool, // OBJ only
}

fn pick_top_two_layers(layers: &[Option<Layer>; 6]) -> (&Layer, Option<&Layer>) {
    let (first, second) = pick_top_two(layers.iter().filter_map(|o| o.as_ref()), |l| l.priority);
    (first.unwrap(), second) // We'll always have at least the backdrop
}

fn render_lcd_line(
    screen_y: u16,
    regs: &LcdControllerRegs,
    vram: &[u8],
    pals: &[u16],
) -> [u16; 240] {
    let bg_vram = &vram[..64 * 1024];
    let bg_pals = &pals[..16 * 16];
    let _obj_vram = &vram[64 * 1024..];
    let bitmap_vram = &vram[..80 * 1024];

    let mut buf = [0; 240];

    for screen_x in 0..240u16 {
        // [OBJ, BG0, BG1, BG2, BG3, backdrop]
        let mut layers = [None; 6];

        // TODO: OBJ support

        // Background layers
        match regs.video_mode {
            0 => render_mode0_backgrounds(&mut layers, screen_y, screen_x, regs, bg_vram, bg_pals),
            1 => render_mode1_backgrounds(&mut layers, screen_y, screen_x, regs, bg_vram, bg_pals),
            2 => unimplemented!(),
            3 => render_mode3_backgrounds(&mut layers, screen_y, screen_x, regs, bitmap_vram),
            4 => render_mode4_backgrounds(
                &mut layers,
                screen_y,
                screen_x,
                regs,
                bitmap_vram,
                bg_pals,
            ),
            5 => render_mode5_backgrounds(&mut layers, screen_y, screen_x, regs, bitmap_vram),
            invalid_mode => println!("Invalid display mode: {}", invalid_mode),
        }

        // Backdrop layer
        layers[5] = Some(Layer {
            id: LayerId::Backdrop,
            color: bg_pals[0],
            priority: 4,
            force_alpha_blend: false,
        });

        let (top_layer, _bottom_layer) = pick_top_two_layers(&layers);
        // TODO: Blending
        let output = top_layer.color;

        buf[screen_x as usize] = output;
    }
    buf
}

fn render_mode0_backgrounds(
    layers: &mut [Option<Layer>; 6],
    screen_y: u16,
    screen_x: u16,
    regs: &LcdControllerRegs,
    bg_vram: &[u8],
    bg_pals: &[u16],
) {
    for bg in 0..=3 {
        if regs.bg_layer_enabled[bg] {
            layers[bg + 1] = render_text_bg_pixel(
                screen_y,
                screen_x,
                bg as u8,
                &regs.bg_attributes[bg],
                bg_vram,
                bg_pals,
            );
        }
    }
}

fn render_mode1_backgrounds(
    layers: &mut [Option<Layer>; 6],
    screen_y: u16,
    screen_x: u16,
    regs: &LcdControllerRegs,
    bg_vram: &[u8],
    bg_pals: &[u16],
) {
    for bg in 0..=1 {
        if regs.bg_layer_enabled[bg] {
            layers[bg + 1] = render_text_bg_pixel(
                screen_y,
                screen_x,
                bg as u8,
                &regs.bg_attributes[bg],
                bg_vram,
                bg_pals,
            );
        }
    }
    // TODO: affine backgrounds
}

const BITMAP_BG_LAYER: usize = 2;

fn render_mode3_bg_pixel(
    screen_y: u16,
    screen_x: u16,
    bg_regs: &BgAttributes,
    vram: &[u8],
) -> Option<Layer> {
    if screen_y >= 160 || screen_x >= 240 {
        return None;
    }

    let pixel_offset = (screen_y * 240 + screen_x) as usize * 2;
    let color = LE::read_u16(&vram[pixel_offset..]);

    Some(Layer {
        id: LayerId::Bg(BITMAP_BG_LAYER as u8),
        color,
        priority: bg_regs.priority,
        force_alpha_blend: false,
    })
}

fn render_mode3_backgrounds(
    layers: &mut [Option<Layer>; 6],
    screen_y: u16,
    screen_x: u16,
    regs: &LcdControllerRegs,
    vram: &[u8],
) {
    if regs.bg_layer_enabled[BITMAP_BG_LAYER] {
        // TODO: affine support
        layers[BITMAP_BG_LAYER] = render_mode3_bg_pixel(
            screen_y,
            screen_x,
            &regs.bg_attributes[BITMAP_BG_LAYER],
            vram,
        );
    }
}

fn render_mode4_bg_pixel(
    screen_y: u16,
    screen_x: u16,
    bg_regs: &BgAttributes,
    display_page: u8,
    vram: &[u8],
    bg_pals: &[u16],
) -> Option<Layer> {
    if screen_y >= 160 || screen_x >= 240 {
        return None;
    }

    let page_offset = (screen_y * 240 + screen_x) as usize;
    let page_base = display_page as usize * 0xA000;

    let palette_index = vram[page_base + page_offset];
    let color = bg_pals[palette_index as usize];

    if palette_index != 0 {
        Some(Layer {
            id: LayerId::Bg(BITMAP_BG_LAYER as u8),
            color,
            priority: bg_regs.priority,
            force_alpha_blend: false,
        })
    } else {
        None
    }
}

fn render_mode4_backgrounds(
    layers: &mut [Option<Layer>; 6],
    screen_y: u16,
    screen_x: u16,
    regs: &LcdControllerRegs,
    vram: &[u8],
    bg_pals: &[u16],
) {
    if regs.bg_layer_enabled[BITMAP_BG_LAYER] {
        // TODO: affine support
        layers[BITMAP_BG_LAYER] = render_mode4_bg_pixel(
            screen_y,
            screen_x,
            &regs.bg_attributes[BITMAP_BG_LAYER],
            regs.active_display_page,
            vram,
            bg_pals,
        );
    }
}

fn render_mode5_bg_pixel(
    screen_y: u16,
    screen_x: u16,
    bg_regs: &BgAttributes,
    display_page: u8,
    vram: &[u8],
) -> Option<Layer> {
    if screen_y >= 128 || screen_x >= 160 {
        return None;
    }

    let page_offset = (screen_y * 160 + screen_x) as usize * 2;
    let page_base = display_page as usize * 0xA000;

    let color = LE::read_u16(&vram[page_base + page_offset..]);

    Some(Layer {
        id: LayerId::Bg(BITMAP_BG_LAYER as u8),
        color,
        priority: bg_regs.priority,
        force_alpha_blend: false,
    })
}

fn render_mode5_backgrounds(
    layers: &mut [Option<Layer>; 6],
    screen_y: u16,
    screen_x: u16,
    regs: &LcdControllerRegs,
    vram: &[u8],
) {
    if regs.bg_layer_enabled[BITMAP_BG_LAYER] {
        // TODO: affine support
        layers[BITMAP_BG_LAYER] = render_mode5_bg_pixel(
            screen_y,
            screen_x,
            &regs.bg_attributes[BITMAP_BG_LAYER],
            regs.active_display_page,
            vram,
        );
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
                let line_buf = render_lcd_line(screen_y as u16, regs, vram, pals);
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

    let mut bgx = 0xC0;
    let mut bgy = 0x40;

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
            vram_mem.as_ref(),
            pal_mem.as_ref(),
        );

        canvas.clear();
        canvas.copy(&lcd_texture, None, None)?;
        canvas.present();
    }

    Ok(())
}
