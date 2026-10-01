"""Generate the application + tray icons.

Design: a full circle (#18181b) with a centred, rounded-cap checkmark (#f4f4f5) — minimal and
monochrome, matching the app UI. Original artwork; no third-party assets or trade dress.
"""

from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
SCALE = 4

DARK = (24, 24, 27, 255)      # #18181b
LIGHT = (244, 244, 245, 255)  # #f4f4f5

# Circle inset as a fraction of the canvas (V2 "roomier circle").
CIRCLE_MARGIN = 0.06
# Centred checkmark (option A proportions), normalised to the canvas.
CHECK = {
    "p0": (0.29, 0.525),
    "elbow": (0.44, 0.665),
    "p2": (0.735, 0.345),
    "width": 0.115,
}


def draw_icon(size: int) -> Image.Image:
    canvas_size = size * SCALE
    image = Image.new("RGBA", (canvas_size, canvas_size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    margin = round(canvas_size * CIRCLE_MARGIN)
    draw.ellipse(
        (margin, margin, canvas_size - margin, canvas_size - margin),
        fill=DARK,
    )

    points = [
        (CHECK["p0"][0] * canvas_size, CHECK["p0"][1] * canvas_size),
        (CHECK["elbow"][0] * canvas_size, CHECK["elbow"][1] * canvas_size),
        (CHECK["p2"][0] * canvas_size, CHECK["p2"][1] * canvas_size),
    ]
    stroke = max(SCALE * 2, round(CHECK["width"] * canvas_size))
    draw.line(points, fill=LIGHT, width=stroke, joint="curve")
    cap = stroke / 2
    for x, y in points:
        draw.ellipse((x - cap, y - cap, x + cap, y + cap), fill=LIGHT)

    return image.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    ASSETS.mkdir(parents=True, exist_ok=True)
    icon = draw_icon(512)
    icon.save(ASSETS / "icon.png", optimize=True)
    icon.save(
        ASSETS / "icon.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    draw_icon(16).save(ASSETS / "tray-icon.png", optimize=True)
    draw_icon(32).save(ASSETS / "tray-icon@2x.png", optimize=True)


if __name__ == "__main__":
    main()
