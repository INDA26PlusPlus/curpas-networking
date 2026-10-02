use chesslib::Board;
use chesslib::IntMove;
use chesslib::piece::Piece;
use ggez::*;
use rand::Rng;
use std::io::*;
use std::net::TcpStream;
use std::net::TcpListener;

// ToDo: Possibly add support for different screen resolutions
static TOP_LEFT: [f32; 2] = [448.0, 28.0];
static GAME_SCALE: f32 = 4.0;
static HARD_FOG_SCALE: f32 = 12.0;
static SOFT_FOG_SCALE: f32 = 5.0;
static PROMOTION_MARGIN: f32 = 30.0;

struct Ember {
    x: f32,
    y: f32,
    brightness: f32,
    color: graphics::Color,
}

struct State {
    dt: std::time::Duration,
    time: f32,
    board: chesslib::Board,
    waiting: bool,
    side: char,
    start: [i8; 2],
    promoting_move: Option<IntMove>,
    promotion_left: bool,
    game_state: i8,
    image_background: graphics::Image,
    image_pieces: graphics::Image,
    image_hints: graphics::Image,
    image_promo: graphics::Image,
    image_eyes: graphics::Image,
    image_ember: graphics::Image,
    hidden_mask: Vec<bool>,
    fog_mask: Vec<u8>,
    embers: Vec<Ember>,

    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

fn upscale_image(ctx: &mut Context, image: &graphics::Image, scale: usize) -> GameResult<graphics::Image> {
    let width = image.width() as usize;
    let height = image.height() as usize;

    let original = image.to_pixels(ctx)?;

    let new_width = width * scale;
    let new_height = height * scale;

    let mut pixels = vec![0u8; new_width * new_height * 4];

    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;

            for dy in 0..scale {
                for dx in 0..scale {
                    let nx = x * scale + dx;
                    let ny = y * scale + dy;

                    let dst = (ny * new_width + nx) * 4;

                    pixels[dst..dst + 4]
                        .copy_from_slice(&original[src..src + 4]);
                }
            }
        }
    }

    Ok(graphics::Image::from_pixels(
        ctx,
        &pixels,
        graphics::ImageFormat::Rgba8UnormSrgb,
        new_width as u32,
        new_height as u32,
    ))
}

fn add_glow(ctx: &mut Context, image: &graphics::Image, radius: i32, strength: f32) -> GameResult<graphics::Image> {
    let width = image.width() as usize;
    let height = image.height() as usize;

    let original = image.to_pixels(ctx)?;
    let mut result = original.clone();

    for y in 0..height {
        for x in 0..width {
            let mut glow_r = 0.0;
            let mut glow_g = 0.0;
            let mut glow_b = 0.0;
            let mut glow_a = 0.0;

            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;

                    if nx < 0 || nx >= width as i32 ||
                       ny < 0 || ny >= height as i32 {
                        continue;
                    }

                    let distance = ((dx * dx + dy * dy) as f32).sqrt();

                    if distance > radius as f32 {
                        continue;
                    }

                    let i = (ny as usize * width + nx as usize) * 4;

                    let alpha = original[i + 3] as f32 / 255.0;

                    // Fade with distance
                    let falloff = 1.0 - distance / radius as f32;

                    let contribution = alpha * falloff * strength;

                    glow_r += original[i] as f32 * contribution;
                    glow_g += original[i + 1] as f32 * contribution;
                    glow_b += original[i + 2] as f32 * contribution;
                    glow_a += contribution;
                }
            }

            let i = (y * width + x) * 4;

            if original[i + 3] == 0 {
                result[i] = glow_r.clamp(0.0, 255.0) as u8;
                result[i + 1] = glow_g.clamp(0.0, 255.0) as u8;
                result[i + 2] = glow_b.clamp(0.0, 255.0) as u8;
                result[i + 3] = (glow_a * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    }

    Ok(graphics::Image::from_pixels(
        ctx,
        &result,
        graphics::ImageFormat::Rgba8UnormSrgb,
        width as u32,
        height as u32,
    ))
}

fn format_move(m: IntMove) -> String {
    let file1 = (b'A' + m.sx as u8) as char;
    let file2 = (b'A' + m.ex as u8) as char;
    let p = match m.promotion_piece {
        Some(Piece::Pawn) => 'P',
        Some(Piece::Knight) => 'N',
        Some(Piece::Rook) => 'R',
        Some(Piece::Bishop) => 'B',
        Some(Piece::Queen) => 'Q',
        Some(Piece::King) => 'K',
        _ => '-',
    };

    format!("{}{}{}{}{}", file1, m.sy + 1, file2, m.ey + 1, p)
}

fn unformat_move(s: String) -> IntMove {
    let sx = (s.chars().nth(0).unwrap() as u8 - b'A') as i8;
    let sy = (s.chars().nth(1).unwrap() as u8 - b'1') as i8;
    let ex = (s.chars().nth(2).unwrap() as u8 - b'A') as i8;
    let ey = (s.chars().nth(3).unwrap() as u8 - b'1') as i8;
    let promotion_piece = match s.chars().nth(4).unwrap() {
        'P' => Some(Piece::Pawn),
        'N' => Some(Piece::Knight),
        'R' => Some(Piece::Rook),
        'B' => Some(Piece::Bishop),
        'Q' => Some(Piece::Queen),
        'K' => Some(Piece::King),
        _ => None,
    };

    IntMove {sy, sx, ey, ex, promotion_piece}
}

impl State {
    fn new(ctx: &mut Context, host: bool, ip: String) -> GameResult<State> {
        let dt = std::time::Duration::new(0, 0);
        let time = 0.0;

        let image_background = graphics::Image::from_path(ctx, "/grass.png")?;
        let image_pieces = graphics::Image::from_path(ctx, "/chess.png")?;
        let image_hints = graphics::Image::from_path(ctx, "/hint.png")?;
        let image_promo = graphics::Image::from_path(ctx, "/promotion.png")?;
        let image_eyes_no_glow = upscale_image(ctx, &graphics::Image::from_path(ctx, "/eyes.png")?, GAME_SCALE as usize)?;
        let image_eyes = add_glow(ctx, &image_eyes_no_glow, 7, 0.04)?;
        let pixels = (0..8 * 8).flat_map(|i| {
                let x = i % 8;
                let y = i / 8;

                if (2..6).contains(&x) && (2..6).contains(&y) {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 0]
                }
            }).collect::<Vec<u8>>();

        let image_ember_no_glow = graphics::Image::from_pixels(ctx, &pixels, graphics::ImageFormat::Rgba8UnormSrgb, 8, 8);
        let image_ember = add_glow(ctx, &image_ember_no_glow, 7, 0.07)?;

        let promoting_move = None;
        let promotion_left = false;
        let game_state = 0; // game_states: 0 -> in play, 1 -> White wins, 2 -> draw, 3 -> Black wins
        let fog_mask = vec![255 as u8; 64];
        let hidden_mask = vec![true; 64];
        let embers: Vec<Ember> = vec![];

        let start = [-1, -1];

        let fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
        let board = match Board::from_fen(fen) {
            Ok(val) => val,
            Err(e) => {
                println!("{e}");
                return Err(ggez::GameError::CustomError(e));
            }
        };

        let mut reader: BufReader<TcpStream>;
        let mut writer: TcpStream;
        let side;
        if host {
            let listener = TcpListener::bind(ip).unwrap();
            let (stream, addr) = listener.accept()?;
            stream.set_nonblocking(true)?;
            println!("Recieved client at addr {addr:?}");
            
            reader = BufReader::new(stream.try_clone()?);
            writer = stream;

            side = match rand::rng().random_bool(0.5) {
                true => 'w',
                false => 'b',
            };

            if side == 'w' {
                writeln!(writer, "B")?;
            }
            else {
                writeln!(writer, "W")?;
            }
        }
        else {
            let stream = TcpStream::connect(ip)?;
            stream.set_nonblocking(true)?;

            reader = BufReader::new(stream.try_clone()?);
            writer = stream;

            let mut msg = String::new();
            while msg == "" {
                let r = reader.read_line(&mut msg);
                match r {
                    Ok(_) => {
                        msg = msg.trim_end().to_string();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {println!("Socket error: {}", e)}    
                }
            }

            if msg == "W" {
                side = 'w';
            }
            else if msg == "B" {
                side = 'b'
            }
            else {
                println!("Unrecognized starting side: {}", msg);
                panic!();
            }
        }

        let waiting = match side {
            'w' => false,
            _ => true,
        };

        Ok(State {dt, time, board, side, waiting, start, promoting_move, promotion_left, game_state, image_background, image_pieces, image_hints, image_promo, image_eyes, image_ember, fog_mask, hidden_mask, embers, reader, writer})
    }

    fn make_move(&mut self, mv: IntMove) {
        let mut b2 = self.board.clone();
        let _ = b2.make_and_validate_move(mv);
        let s = format!("{}{}", format_move(mv), b2.get_board_representation());
        let _ = writeln!(self.writer, "{s}");
        println!("{s}");

        let mut msg = String::new();
        while msg == "" {
            let r = self.reader.read_line(&mut msg);
            match r {
                Ok(_) => {
                    msg = msg.trim_end().to_string();
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {println!("Socket error: {}", e)}    
            }
        }

        if msg == "OK" || msg == "CHECKMATE" || msg == "STALEMATE" {
            let _ = self.board.make_and_validate_move(mv);
            if self.board.is_checkmate() {
                self.game_state = match self.side {
                    'w' => 1,
                    _ => 3,
                }
            }
            else if self.board.is_stalemate() {
                self.game_state = 2;
            }
            else {
                self.waiting = true;
            }
        }
        else {
            println!("Opponent did not accept move. msg: {}", msg);
        }
    }
}

fn smoothstep(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

const PERM: [u8; 512] = {
    let p: [u8; 256] = [
        151,160,137,91,90,15,131,13,201,95,96,53,194,233,7,225,
        140,36,103,30,69,142,8,99,37,240,21,10,23,190,6,148,247,
        120,234,75,0,26,197,62,94,252,219,203,117,35,11,32,57,177,
        33,88,237,149,56,87,174,20,125,136,171,168,68,175,74,165,
        71,134,139,48,27,166,77,146,158,231,83,111,229,122,60,211,
        133,230,220,105,92,41,55,46,245,40,244,102,143,54,65,25,
        63,161,1,216,80,73,209,76,132,187,208,89,18,169,200,196,
        135,130,116,188,159,86,164,100,109,198,173,186,3,64,52,217,
        226,250,124,123,5,202,38,147,118,126,255,82,85,212,207,206,
        59,227,47,16,58,17,182,189,28,42,223,183,170,213,119,248,
        152,2,44,154,163,70,221,153,101,155,167,43,172,9,129,22,
        39,253,19,98,108,79,113,224,232,178,185,112,104,218,246,
        97,228,251,34,242,193,238,210,144,12,191,179,162,241,81,
        51,145,235,249,14,239,107,49,192,214,31,181,199,106,157,184,
        84,204,176,115,121,50,45,127,4,150,254,138,236,205,93,222,
        114,67,29,24,72,243,141,128,195,78,66,215,61,156,180,33
    ];

    let mut result = [0u8; 512];
    let mut i = 0;

    while i < 512 {
        result[i] = p[i & 255];
        i += 1;
    }

    result
};

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

fn grad(hash: u8, x: f32, y: f32, z: f32) -> f32 {
    let h = hash & 15;

    let u = if h < 8 { x } else { y };

    let v = if h < 4 {
        y
    } else if h == 12 || h == 14 {
        x
    } else {
        z
    };

    let u = if h & 1 == 0 { u } else { -u };
    let v = if h & 2 == 0 { v } else { -v };

    u + v
}

pub fn noise(x: f32, y: f32, z: f32) -> f32 {
    let xi = x.floor() as i32 & 255;
    let yi = y.floor() as i32 & 255;
    let zi = z.floor() as i32 & 255;

    let xf = x - x.floor();
    let yf = y - y.floor();
    let zf = z - z.floor();

    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);

    let a  = PERM[xi as usize] as usize + yi as usize;
    let aa = PERM[a & 255] as usize + zi as usize;
    let ab = PERM[(a + 1) & 255] as usize + zi as usize;

    let b  = PERM[(xi + 1) as usize] as usize + yi as usize;
    let ba = PERM[b & 255] as usize + zi as usize;
    let bb = PERM[(b + 1) & 255] as usize + zi as usize;

    let x1 = lerp(
        grad(PERM[aa & 255], xf,     yf,     zf),
        grad(PERM[ba & 255], xf - 1.0, yf,     zf),
        u,
    );

    let x2 = lerp(
        grad(PERM[ab & 255], xf,     yf - 1.0, zf),
        grad(PERM[bb & 255], xf - 1.0, yf - 1.0, zf),
        u,
    );

    let y1 = lerp(x1, x2, v);

    let x1 = lerp(
        grad(PERM[(aa + 1) & 255], xf,     yf,     zf - 1.0),
        grad(PERM[(ba + 1) & 255], xf - 1.0, yf,     zf - 1.0),
        u,
    );

    let x2 = lerp(
        grad(PERM[(ab + 1) & 255], xf,     yf - 1.0, zf - 1.0),
        grad(PERM[(bb + 1) & 255], xf - 1.0, yf - 1.0, zf - 1.0),
        u,
    );

    let y2 = lerp(x1, x2, v);

    (lerp(y1, y2, w) * 0.5)+0.5
}

fn hard_fog_pixel(x: u32, y: u32, t: f32, fog_mask: Vec<u8>) -> (u8, u8, u8, u8) {
    let brightness = ((noise((x as f32)*HARD_FOG_SCALE*0.004, (y as f32)*HARD_FOG_SCALE*0.004, -t*0.00006))/10.0) + 0.1;
    let br = (brightness*255.0).clamp(0.0, 255.0) as u8;
    let mut a = 255u8;
    
    let scale_x = ((x as f32) - (TOP_LEFT[0] / HARD_FOG_SCALE)) / (32.0 * (GAME_SCALE/HARD_FOG_SCALE)) - 0.5;
    let scale_y = ((y as f32) - (TOP_LEFT[1] / HARD_FOG_SCALE)) / (32.0 * (GAME_SCALE/HARD_FOG_SCALE)) - 0.5;

    if scale_x >= -0.5 && scale_x < 7.5 && scale_y >= -0.5 && scale_y < 7.5 {
        let mut op_tl = 255u8;
        let mut op_tr = 255u8;
        let mut op_bl = 255u8;
        let mut op_br = 255u8;
        if scale_x >= 0.0 && scale_y >= 0.0 {
            op_tl = fog_mask[(7 - (scale_y.floor() as usize)) * 8 + (scale_x.floor() as usize)];
        }
        if scale_x <= 7.0 && scale_y >= 0.0 {
            op_tr = fog_mask[(7 - (scale_y.floor() as usize)) * 8 + (scale_x.ceil() as usize)];
        }
        if scale_x >= 0.0 && scale_y <= 7.0 {
            op_bl = fog_mask[(7 - (scale_y.ceil() as usize)) * 8 + (scale_x.floor() as usize)];
        }
        if scale_x <= 7.0 && scale_y <= 7.0 {
            op_br = fog_mask[(7 - (scale_y.ceil() as usize)) * 8 + (scale_x.ceil() as usize)];
        }

        let dx = smoothstep(scale_x - scale_x.floor());
        let dy = smoothstep(scale_y - scale_y.floor());
        a = ((
            (op_tl as f32) * (1.0 - dx) * (1.0 - dy) +
            (op_tr as f32) * (dx) * (1.0 - dy) +
            (op_bl as f32) * (1.0 - dx) * (dy) +
            (op_br as f32) * (dx) * (dy)) * 2.0)
            .clamp(0.0, 255.0) as u8;
    }

    (br, br, br, a)
}

fn soft_fog_pixel(x: u32, y: u32, t: f32) -> (u8, u8, u8, u8) {
    let r = 120u8;
    let g = 120u8;
    let b = 120u8;

    let a2: f32 = noise((x as f32)*SOFT_FOG_SCALE*0.003 + 0.00015*t, (y as f32)*SOFT_FOG_SCALE*0.012 + 0.00015*t, 0.00006*t)-0.5;
    let a_out = (a2*255.0).clamp(0.0, 255.0) as u8;

    (r, g, b, a_out)
}

impl ggez::event::EventHandler for State {
    fn mouse_button_down_event(&mut self, _ctx: &mut Context, button: input::mouse::MouseButton, x: f32, y: f32) -> GameResult {
        if button == input::mouse::MouseButton::Left {
            // Clicked on board
            if TOP_LEFT[0] < x && x < TOP_LEFT[0] + (256.0*GAME_SCALE) && TOP_LEFT[1] < y && y < TOP_LEFT[1] + (256.0*GAME_SCALE) && (x - TOP_LEFT[0]) % (32.0*GAME_SCALE) > 1.0*GAME_SCALE  && (x - TOP_LEFT[0]) % (32.0*GAME_SCALE) < 31.0*GAME_SCALE  && (y - TOP_LEFT[1]) % (32.0*GAME_SCALE) > 1.0*GAME_SCALE  && (y - TOP_LEFT[1]) % (32.0*GAME_SCALE) < 31.0*GAME_SCALE {
                if !self.waiting {
                    let tx = (x - TOP_LEFT[0]) / (32.0*GAME_SCALE);
                    let mx = (match self.side {'b' => 8.0 - tx, _ => tx}).floor() as i8;
                    let ty = (y - TOP_LEFT[1]) / (32.0*GAME_SCALE);
                    let my = (match self.side {'w' => 8.0 - ty, _ => ty}).floor() as i8;
                    let mut is_start = false;
                    for m in self.board.valid_moves.clone() {
                        if m.sx == mx && m.sy == my {
                            self.start = [mx, my];
                            is_start = true;
                            break;
                        }
                    }
                    if !is_start {
                        if self.start != [-1, -1] {
                            for mut m in self.board.valid_moves.clone() {
                                if m.ex == mx && m.ey == my && m.sx == self.start[0] && m.sy == self.start[1] {
                                    if (self.board.get_int_board()[m.sy as usize][m.sx as usize] == 0 && m.ey == 7) || (self.board.get_int_board()[m.sy as usize][m.sx as usize] == 6 && m.ey == 0) {
                                        m.promotion_piece = None;
                                        self.promoting_move = Some(m);
                                        self.promotion_left = match match self.side {'w' => m.ex, _ => 7 - m.ex} / 4 {
                                            0 => true,
                                            _ => false,
                                        };
                                    }
                                    else {
                                        self.make_move(m);
                                    }
                                    break;
                                }
                            }
                            self.start = [-1, -1];
                        }
                    }
                }
            }
            // Clicked on promotion selector
            else {
                let left = match self.promotion_left {
                    true => TOP_LEFT[0] - PROMOTION_MARGIN - (32.0*GAME_SCALE),
                    _ => TOP_LEFT[0] + PROMOTION_MARGIN + ((256.0 + 32.0)*GAME_SCALE),
                };
                if left < x && x < left + (32.0*GAME_SCALE) && TOP_LEFT[1] < y && y < TOP_LEFT[1] + (128.0*GAME_SCALE) {
                    let height = ((y - (TOP_LEFT[1])) / (32.0*GAME_SCALE)).floor();
                    if self.promoting_move != None && self.promoting_move.unwrap().promotion_piece == None {
                        self.promoting_move = Some(IntMove {sx: self.promoting_move.unwrap().sx, sy: self.promoting_move.unwrap().sy, ex: self.promoting_move.unwrap().ex, ey: self.promoting_move.unwrap().ey, promotion_piece: match height{
                            1.0 => Some(Piece::Knight),
                            2.0 => Some(Piece::Bishop),
                            3.0 => Some(Piece::Rook),
                            _ => Some(Piece::Queen),
                        }});
                    }
                    self.make_move(self.promoting_move.unwrap());
                    self.promoting_move = None;
                }
            }
        }

        Ok(())
    }

    fn update(&mut self, ctx: &mut Context) -> GameResult {
        self.dt = ctx.time.delta();
        self.time += (self.dt.as_nanos() as f32) / 1000_000.0;

        if self.waiting {
            let mut msg = String::new();
            let r = self.reader.read_line(&mut msg);
            match r {
                Ok(_) => {
                    msg = msg.trim_end().to_string();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {println!("Socket error: {}", e)}    
            }
            if msg.len() == 69 {
                let (msg_mv, msg_b) = msg.split_at(5);
                let mv = unformat_move(msg_mv.to_string());
                let mut b2 = self.board.clone();
                let r = b2.make_and_validate_move(mv);
                let b = b2.get_board_representation().trim_end().to_string();
                match r {
                    Ok(_) => {
                        if msg_b.to_string() != b {
                            println!("Communication error, boards {} and {} do not match", msg_b, b);
                            let _ = writeln!(self.writer, "REJECT");
                            println!("REJECT");
                        }
                        else {
                            self.board.make_and_validate_move(mv);
                            if self.board.is_checkmate() {
                                self.game_state = match self.side {
                                    'w' => 3,
                                    _ => 1,
                                };
                                let _ = writeln!(self.writer, "CHECKMATE");
                                println!("CHECKMATE");
                            }
                            else if self.board.is_stalemate() {
                                self.game_state = 2;
                                let _ = writeln!(self.writer, "STALEMATE");
                                println!("STALEMATE");
                            }
                            else {
                                self.waiting = false;
                                let _ = writeln!(self.writer, "OK");
                                println!("OK");
                            }
                        }
                    }
                    Err(_e) => {
                        let _ = writeln!(self.writer, "REJECT");
                        println!("REJECT");
                    }
                }
            }
        }

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        // println!("dt: {}.{}ms", self.dt.as_nanos() / 1000000, self.dt.as_nanos() / 10000 - (100 * (self.dt.as_nanos() / 1000000)));
        let mut canvas = graphics::Canvas::from_frame(ctx, graphics::Color::from_rgb(20, 35, 20));
        canvas.set_sampler(graphics::Sampler::nearest_clamp());
        let screen = ctx.gfx.drawable_size();

        // Background

        canvas.draw(
            &self.image_background,
            graphics::DrawParam::new()
                .dest(TOP_LEFT)
                .scale([GAME_SCALE, GAME_SCALE])
                .color(graphics::Color::new(0.6, 0.6, 0.6, 1.0))
        );

        // Pieces Layer 1

        let int_board = self.board.get_int_board();
        for r in (0..8).rev() {
            for c in 0usize..8 {
                if int_board[r][c] == 12 {
                    continue;
                }
                let sheet_coord = match int_board[r][c] % 6 {
                    0 => [0.0,3.0],
                    1 => [3.0,3.0],
                    2 => [1.0,3.0],
                    3 => [2.0,3.0],
                    4 => [4.0,3.0],
                    5 => [5.0,3.0],
                    _ => [0.0,0.0],
                };
                canvas.draw(
                    &self.image_pieces,
                    graphics::DrawParam::new()
                        .src(graphics::Rect::new(
                            (sheet_coord[0] * 32.0) / 192.0,
                            (sheet_coord[1] * 32.0) / 128.0,
                            32.0 / 192.0,
                            32.0 / 128.0,
                        ))
                        .dest([TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)), TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32))])
                        .scale([GAME_SCALE, GAME_SCALE])
                );
            }
        }

        // Embers and visible tiles

        for r in (0..8).rev() {
            for c in 0usize..8 {
                if int_board[r][c] == 12 || int_board[r][c] % 6 == 2 || int_board[r][c] % 6 == 4 {
                    continue;
                }
                if ((self.side == 'w' && int_board[r][c] >= 6) || (self.side == 'b' && int_board[r][c] <= 5)) && self.game_state == 0{
                    continue;
                }
                if int_board[r][c] % 6 == 0 {
                    let p = 1.0 - (28.0_f32 / 30.0).powf((self.dt.as_nanos() as f32 / 1000_000_000.0) * 30.0);
                    if rand::rng().random_bool(p as f64) {
                        self.embers.push(Ember {
                            x: TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)) + 7.0*GAME_SCALE,
                            y: TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32)) + 24.0*GAME_SCALE,
                            color: graphics::Color::new(rand::rng().random::<f32>() * 0.5 + 0.5, rand::rng().random::<f32>() * 0.2 + 0.25, 0.25, 1.0),
                            brightness: 1.0,
                        });
                    }
                }
                else if int_board[r][c] % 6 == 1 {
                    let p = 1.0 - (26.0_f32 / 30.0).powf((self.dt.as_nanos() as f32 / 1000_000_000.0) * 30.0);
                    if rand::rng().random_bool(p as f64) {
                        self.embers.push(Ember {
                            x: TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)) + 14.0*GAME_SCALE,
                            y: TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32)) + 22.0*GAME_SCALE,
                            color: graphics::Color::new(rand::rng().random::<f32>() * 0.5 + 0.5, rand::rng().random::<f32>() * 0.2 + 0.25, 0.25, 1.0),
                            brightness: 1.0,
                        });
                    }
                }
                else if int_board[r][c] % 6 == 3 {
                    let p = 1.0 - (26.0_f32 / 30.0).powf((self.dt.as_nanos() as f32 / 1000_000_000.0) * 30.0);
                    if rand::rng().random_bool(p as f64) {
                        self.embers.push(Ember {
                            x: TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)) + 15.0*GAME_SCALE,
                            y: TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32)) + 22.0*GAME_SCALE,
                            color: graphics::Color::new(rand::rng().random::<f32>() * 0.5 + 0.5, rand::rng().random::<f32>() * 0.2 + 0.25, 0.25, 1.0),
                            brightness: 1.0,
                        });
                    }
                }
                else if int_board[r][c] % 6 == 5 {
                    let p = 1.0 - (24.0_f32 / 30.0).powf((self.dt.as_nanos() as f32 / 1000_000_000.0) * 30.0);
                    if rand::rng().random_bool(p as f64) {
                        let mut c4 = graphics::Color::new(rand::rng().random::<f32>() * 0.4 + 0.6, rand::rng().random::<f32>() * 0.2 + 0.35, 0.35, 1.0);
                        if self.game_state == 1 {
                            if int_board[r][c] == 5 {
                                c4 = graphics::Color::new(rand::rng().random::<f32>() * 0.2 + 0.1, rand::rng().random::<f32>() * 0.2 + 0.75, 0.25, 1.0)
                            }
                            else {
                                c4 = graphics::Color::new(0.0, 0.0, 0.0, 1.0)
                            }
                        }
                        else if self.game_state == 2 {
                            c4 = graphics::Color::new(rand::rng().random::<f32>() * 0.2 + 0.65, rand::rng().random::<f32>() * 0.2 + 0.65, 0.35, 1.0)
                        }
                        else if self.game_state == 3 {
                            if int_board[r][c] == 11 {
                                c4 = graphics::Color::new(rand::rng().random::<f32>() * 0.2 + 0.1, rand::rng().random::<f32>() * 0.2 + 0.75, 0.25, 1.0)
                            }
                            else {
                                c4 = graphics::Color::new(0.0, 0.0, 0.0, 1.0)
                            }
                        }
                        self.embers.push(Ember {
                            x: TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)) + 13.5*GAME_SCALE,
                            y: TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32)) + 16.0*GAME_SCALE,
                            color: c4,
                            brightness: 1.0,
                        });
                    }
                }
            }
        }

        if self.game_state != 0 {
            for i in 0..64usize {
                self.hidden_mask[i] = false;
            }
        }

        if !self.waiting {
            for i in 0..64usize {
                self.hidden_mask[i] = true;
            }

            if self.game_state != 0 {
                for i in 0..64usize {
                    self.hidden_mask[i] = false;
                }
            }

            let mut highlights: Vec<i8> = vec![];
            if self.game_state == 0 {
                let mut show = self.board.valid_moves.clone();
                let c = match self.side {'w' => chesslib::utils::Colour::White, _ => chesslib::utils::Colour::Black};
                let b = self.board.get_int_board();
                let mut k_sq = 0i8;
                for r in 0..8usize {
                    for c in 0..8usize {
                        if (b[r][c] == 5 && self.side == 'w') || (b[r][c] == 11 && self.side == 'b') {
                            k_sq = (r * 8 + c) as i8;
                        }
                        if (self.side == 'w' && b[r][c] <= 5) || (self.side == 'b' && b[r][c] >= 6 && b[r][c] != 12) {
                            show.push(IntMove {sx: c as i8, sy: r as i8, ex: c as i8, ey: r as i8, promotion_piece: None});
                        }
                    }
                }
                let mut dir_y: Vec<i8> = vec![0i8; 1];
                if k_sq / 8 != 0 {
                    dir_y.push(-1i8);
                }
                if k_sq / 8 != 7 {
                    dir_y.push(1i8);
                }
                let mut dir = vec![[0i8, 0i8]; 0];
                for d in dir_y {
                    dir.push([0, d]);
                    if k_sq % 8 != 0 {
                        dir.push([-1, d]);
                    }
                    if k_sq % 8 != 7 {
                        dir.push([1, d]);
                    }
                }

                for ds in dir {
                    let p = b[(k_sq / 8 + ds[1]) as usize][(k_sq % 8 + ds[0]) as usize];
                    if ((p > 5 && self.side == 'w') || ((p < 6 || p == 12) && self.side == 'b')) && !self.board.valid_moves.contains(&IntMove {sx: k_sq % 8, sy: k_sq / 8, ex: k_sq % 8 + ds[0], ey: k_sq / 8 + ds[1], promotion_piece: None}) {
                        show.push(IntMove {sx: k_sq % 8 + ds[0], sy: (k_sq / 8) + ds[1], ex: k_sq % 8 + ds[0], ey: (k_sq / 8) + ds[1], promotion_piece: None});
                        highlights.push(k_sq + (ds[1] * 8) + ds[0]);
                    }
                    else if self.board.is_attacked(&c, k_sq % 8 + ds[0], (k_sq / 8) + ds[1]) {
                        show.push(IntMove {sx: k_sq % 8 + ds[0], sy: (k_sq / 8) + ds[1], ex: k_sq % 8 + ds[0], ey: (k_sq / 8) + ds[1], promotion_piece: None});
                        highlights.push(k_sq + (ds[1] * 8) + ds[0]);
                    }
                }

                if self.board.in_check(&c) {
                    let checkers = self.board.get_checker(&c);
                    for checker in checkers {
                        show.push(IntMove {sx: checker.0 as i8, sy: checker.1 as i8, ex: checker.0 as i8, ey: checker.1 as i8, promotion_piece: None});
                        highlights.push((checker.1 * 8 + checker.0) as i8);
                    }
                    show.push(IntMove {sx: k_sq % 8, sy: k_sq / 8 as i8, ex: k_sq % 8 as i8, ey: k_sq / 8 as i8, promotion_piece: None});
                    highlights.push((k_sq) as i8);
                }

                for m in show {
                    self.hidden_mask[m.ey as usize * 8 + m.ex as usize] = false;
                }
            }
            
            for h in highlights {
                let p = 1.0 - (20.0_f32 / 30.0).powf((self.dt.as_nanos() as f32 / 1000_000_000.0) * 30.0);
                if rand::rng().random_bool(p as f64) {
                    self.embers.push(Ember {
                        x: TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-h%8, _ => h%8}) as f32)) + rand::rng().random::<f32>()*32.0*GAME_SCALE,
                        y: TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-h/8, _ => h/8}) as f32)) + rand::rng().random::<f32>()*32.0*GAME_SCALE,
                        color: graphics::Color::new(rand::rng().random::<f32>() * 0.7 + 0.3, rand::rng().random::<f32>() * 0.2 + 0.25, 0.25, 1.0),
                        brightness: 1.0,
                    });
                }
            }
        }

        for i in 0..64usize {
            let idx = match self.side {'w' => i, _ => 63 - i};
            if self.hidden_mask[i] {
                self.fog_mask[idx] = 255.0_f32.max((self.fog_mask[idx] as f32) * (3.33_f32.powf((self.dt.as_nanos() as f32) / 1000_000_000.0))).floor() as u8;
            }
            else {
                self.fog_mask[idx] = 0.0_f32.max(((self.fog_mask[idx]) as f32) * (0.3_f32.powf((self.dt.as_nanos() as f32) / 1000_000_000.0))).floor() as u8;
            }
        }

        // Update the position and brightness of all embers and draw them
        for e in &mut self.embers {
            // Update
            e.x += (rand::rng().random::<f32>() - 0.5) * 40.0 * (((self.dt.as_nanos()) as f32) / 1000_000_000.0);
            e.y += -1.4 * 6.0 * (((self.dt.as_nanos()) as f32) / 1000_000_000.0);
            e.brightness *= 0.7_f32.powf((self.dt.as_nanos() as f32) / 1000_000_000.0);

            // Draw
            let c3 = graphics::Color::new((e.color.r - 0.2) * e.brightness + 0.2, e.color.g * e.brightness, (e.color.b + 0.3) * e.brightness - 0.3, e.color.a * e.brightness);
            canvas.draw(
                &self.image_ember,
                graphics::DrawParam::new()
                    .color(c3)
                    .dest([e.x, e.y])
                    .scale([GAME_SCALE/2.0, GAME_SCALE/2.0])
            );
        }

        // Destroy dead embers
        self.embers.retain(|e| e.brightness > 0.2);

        // Pieces layer 2

        for r in (0..8).rev() {
            for c in 0usize..8 {
                if int_board[r][c] == 12 {
                    continue;
                }
                let mut sheet_coord = match int_board[r][c] % 6 {
                    0 => [0.0,0.0],
                    1 => [3.0,0.0],
                    2 => [1.0,0.0],
                    3 => [2.0,0.0],
                    4 => [4.0,0.0],
                    5 => [5.0,0.0],
                    _ => [0.0,0.0],
                };
                let colour = match int_board[r][c] {
                    x if x >= 6 => true,
                    _ => false,
                };
                if colour {
                    sheet_coord = [sheet_coord[0], sheet_coord[1] + 1.0];
                }
                canvas.draw(
                    &self.image_pieces,
                    graphics::DrawParam::new()
                        .src(graphics::Rect::new(
                            (sheet_coord[0] * 32.0) / 192.0,
                            (sheet_coord[1] * 32.0) / 128.0,
                            32.0 / 192.0,
                            32.0 / 128.0,
                        ))
                        .dest([TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)), TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32))])
                        .scale([GAME_SCALE, GAME_SCALE])
                );
            }
        }

        // Move hints

        if self.start != [-1, -1] {
            for m in self.board.valid_moves.clone() {
                if m.sx == self.start[0] && m.sy == self.start[1] && (m.promotion_piece == None || m.promotion_piece == Some(Piece::Queen)) {
                    
                    canvas.draw(
                        &self.image_hints,
                        graphics::DrawParam::new()
                            .dest([TOP_LEFT[0] + (32.0*GAME_SCALE*(match self.side {'b' => 7-m.ex, _ => m.ex} as f32)), TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-m.ey, _ => m.ey}) as f32))])
                            .color(graphics::Color::new(1.0, 1.0, 1.0, 0.5))
                            .scale([GAME_SCALE, GAME_SCALE])
                    );
                }
            }
        }

        // Hard fog

        let width = (screen.0 as u32) / (HARD_FOG_SCALE as u32);
        let height = (screen.1 as u32) / (HARD_FOG_SCALE as u32);

        let mut pixels = Vec::with_capacity((width * height * 4) as usize);

        for y in 0..height {
            for x in 0..width {
                let (r, g, b, a) = hard_fog_pixel(x, y, self.time, self.fog_mask.clone());

                pixels.push(r);
                pixels.push(g);
                pixels.push(b);
                pixels.push(a);
            }
        }

        let image = graphics::Image::from_pixels(
            ctx,
            &pixels,
            graphics::ImageFormat::Rgba8UnormSrgb,
            width,
            height,
        );

        canvas.draw(
            &image,
            graphics::DrawParam::default().scale([HARD_FOG_SCALE, HARD_FOG_SCALE]),
        );

        // Eyes

        for r in (0..8).rev() {
            for c in 0usize..8 {
                if int_board[r][c] == 4 {
                    let mut c2 = graphics::Color::new(1.0, 1.0, 1.0, 1.0);
                    if self.side == 'b' && self.game_state == 0 {
                        c2 = graphics::Color::new(1.0, 0.0, 0.0, 1.0);
                    }
                    canvas.draw(
                        &self.image_eyes,
                        graphics::DrawParam::new()
                            .color(c2)
                            .dest([TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)), TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32))])
                    );
                }
                else if int_board[r][c] == 10 {
                    let mut c2 = graphics::Color::new(1.0, 1.0, 1.0, 1.0);
                    if self.side == 'w' && self.game_state == 0 {
                        c2 = graphics::Color::new(1.0, 0.0, 0.0, 1.0);
                    }
                    canvas.draw(
                        &self.image_eyes,
                        graphics::DrawParam::new()
                            .color(c2)
                            .dest([TOP_LEFT[0] + (32.0*GAME_SCALE*((match self.side {'b' => 7-c, _ => c}) as f32)), TOP_LEFT[1] + (32.0*GAME_SCALE*((match self.side {'w' => 7-r, _ => r}) as f32))])
                    );
                }
            }
        }

        if self.promoting_move != None {
            // Draw Choices
            let left = match self.promotion_left {
                true => TOP_LEFT[0] - PROMOTION_MARGIN - (32.0*GAME_SCALE),
                _ => TOP_LEFT[0] + PROMOTION_MARGIN + ((256.0 + 32.0)*GAME_SCALE),
            };
            canvas.draw(
                &self.image_promo,
                graphics::DrawParam::new()
                    .src(graphics::Rect::new(
                        (match self.side {'w' => 0.0, _ => 32.0}) / 64.0,
                        0.0 / 128.0,
                        32.0 / 64.0,
                        128.0 / 128.0,
                    ))
                    .dest([left, TOP_LEFT[1]])
                    .scale([GAME_SCALE, GAME_SCALE])
            );
        }

        // Soft Fog

        let width = (screen.0 as u32) / (SOFT_FOG_SCALE as u32);
        let height = (screen.1 as u32) / (SOFT_FOG_SCALE as u32);

        let mut pixels = Vec::with_capacity((width * height * 4) as usize);

        for y in 0..height {
            for x in 0..width {
                let (r, g, b, a) = soft_fog_pixel(x, y, self.time);

                pixels.push(r);
                pixels.push(g);
                pixels.push(b);
                pixels.push(a);
            }
        }

        let image = graphics::Image::from_pixels(
            ctx,
            &pixels,
            graphics::ImageFormat::Rgba8UnormSrgb,
            width,
            height,
        );

        canvas.draw(
            &image,
            graphics::DrawParam::default().scale([SOFT_FOG_SCALE, SOFT_FOG_SCALE]),
        );

        canvas.finish(ctx)?;
        Ok(())
    }
}

fn main() -> GameResult {
    let args: Vec<String> = std::env::args().collect();
    let host: bool = args.contains(&"--host".to_string());
    let port = 6767;
    let ip: String;
    if !host {
        if !args.contains(&"--ip".to_string()) {
            println!("Cannot initate without --ip or --host");
            panic!();
        }
        else {
            ip = format!("{}:{}", args[2], port);
        }
    }
    else {
        ip = format!("0.0.0.0:{}", port);
    }
    let c = conf::Conf::new();
    let (mut ctx, event_loop) = ContextBuilder::new("Chess", "Curpas")
        .default_conf(c)
        .add_resource_path("./assets")
        .window_setup(
            conf::WindowSetup::default()
                .title(&format!("The Fog Is Coming ({})", match host {true => "host", false => "client"}))
        )
        .window_mode(
            conf::WindowMode::default()
                .fullscreen_type(conf::FullscreenType::Desktop)
        )
        .build()
        .unwrap();
    let state = State::new(&mut ctx, host, ip)?;
    let _ = event::run(ctx, event_loop, state);
    return Ok(())
}