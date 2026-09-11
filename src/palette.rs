pub type Rgb = (u8, u8, u8);

pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub sep: Rgb,
    pub add_bg: Rgb,
    pub del_bg: Rgb,
    pub add_fg: Rgb,
    pub del_fg: Rgb,
    pub active_bg: Rgb,
    pub sel_bg: Rgb,
}

pub const DARK: Palette = Palette {
    bg: (38, 38, 38),
    fg: (232, 232, 232),
    dim: (140, 140, 140),
    sep: (170, 170, 170),
    add_bg: (0, 97, 0),
    del_bg: (105, 0, 0),
    add_fg: (78, 186, 101),
    del_fg: (255, 107, 128),
    active_bg: (58, 58, 58),
    sel_bg: (38, 79, 120),
};

pub const LIGHT: Palette = Palette {
    bg: (242, 242, 242),
    fg: (30, 30, 30),
    dim: (120, 120, 120),
    sep: (120, 120, 120),
    add_bg: (190, 240, 195),
    del_bg: (255, 205, 210),
    add_fg: (30, 130, 50),
    del_fg: (200, 40, 60),
    active_bg: (222, 222, 222),
    sel_bg: (173, 214, 255),
};

pub fn for_dark(dark: bool) -> &'static Palette {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}
