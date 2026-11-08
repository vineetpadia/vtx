"""Rasterize VTX's simple SVG geometry into Windows icon sizes. Requires Pillow."""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / 'assets'

def icon(size):
    scale = 4 * max(1, 256 // size)
    image = Image.new('RGBA', (size * scale, size * scale))
    draw = ImageDraw.Draw(image)
    unit = size * scale / 256
    def points(values):
        return [(x * unit, y * unit) for x, y in values]
    draw.rounded_rectangle((8*unit, 8*unit, 248*unit, 248*unit), radius=54*unit, fill='#102331')
    draw.polygon(points([(44,64),(87,64),(128,160),(128,204),(107,188)]), fill='#F1F7FA')
    draw.polygon(points([(128,160),(169,64),(212,64),(149,188),(128,204)]), fill='#37DFDC')
    return image.resize((size, size), Image.Resampling.LANCZOS)

if __name__ == '__main__':
    ASSETS.mkdir(exist_ok=True)
    for size in [64, 256]:
        icon(size).save(ASSETS / f'vtx-{size}.png')
    sizes = [16,20,24,32,40,48,64,96,128,256]
    icon(256).save(ASSETS / 'vtx.ico', sizes=[(n,n) for n in sizes], append_images=[icon(n) for n in sizes[:-1]])
