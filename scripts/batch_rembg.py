import os
import glob
from PIL import Image
import rembg
import io

SPRITE_DIR = r"E:\19-AI项目\删除文件助手"
COLS = 5
ROWS = 3

def remove_background(img_path, out_path):
    print(f"Processing {img_path} with rembg ...")
    try:
        img = Image.open(img_path).convert("RGBA")
        
        # Calculate target height (e.g. 250px per frame * 3 rows = 750px total)
        # We'll use 1500px total to keep some extra sharpness (500px per frame)
        target_height = 1500
        aspect_ratio = img.width / img.height
        target_width = int(target_height * aspect_ratio)
        
        # Resize image for much faster and safer processing
        img = img.resize((target_width, target_height), Image.Resampling.LANCZOS)
        
        # Convert to bytes for rembg
        byte_arr = io.BytesIO()
        img.save(byte_arr, format='PNG')
        input_data = byte_arr.getvalue()
        
        # Remove background
        output_data = rembg.remove(input_data)
        
        with open(out_path, 'wb') as o:
            o.write(output_data)
            
        print(f"Saved {out_path}")
    except Exception as e:
        print(f"Failed to process {img_path}: {e}")

def main():
    files = glob.glob(os.path.join(SPRITE_DIR, "*_spritesheet.png"))
    for f in files:
        if f.endswith("_transparent.png"):
            continue
        out_path = f.replace(".png", "_transparent.png")
        remove_background(f, out_path)
        
if __name__ == "__main__":
    main()
