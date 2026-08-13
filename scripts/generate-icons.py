from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
SCALE = 4


def draw_icon(size: int) -> Image.Image:
    canvas_size = size * SCALE
    image = Image.new("RGBA", (canvas_size, canvas_size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    margin = round(canvas_size * 0.055)
    radius = round(canvas_size * 0.22)
    draw.rounded_rectangle(
        (margin, margin, canvas_size - margin, canvas_size - margin),
        radius=radius,
        fill=(7, 16, 29, 255),
        outline=(38, 55, 77, 255),
        width=max(SCALE, round(canvas_size * 0.018)),
    )

    center = canvas_size / 2
    ring_radius = canvas_size * 0.305
    ring_width = max(SCALE * 2, round(canvas_size * 0.075))
    draw.ellipse(
        (
            center - ring_radius,
            center - ring_radius,
            center + ring_radius,
            center + ring_radius,
        ),
        outline=(94, 106, 210, 255),
        width=ring_width,
    )

    check_width = max(SCALE * 2, round(canvas_size * 0.085))
    check_points = [
        (canvas_size * 0.31, canvas_size * 0.52),
        (canvas_size * 0.445, canvas_size * 0.655),
        (canvas_size * 0.70, canvas_size * 0.365),
    ]
    draw.line(check_points, fill=(247, 248, 248, 255), width=check_width, joint="curve")
    endpoint_radius = check_width / 2
    for x, y in check_points:
        draw.ellipse(
            (x - endpoint_radius, y - endpoint_radius, x + endpoint_radius, y + endpoint_radius),
            fill=(247, 248, 248, 255),
        )

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
