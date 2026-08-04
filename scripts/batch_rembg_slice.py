import os
import glob
from PIL import Image
import rembg
import io

SPRITE_DIR = r"E:\19-AI项目\删除文件助手"
COLS = 5
ROWS = 3

def process_spritesheet(img_path, out_path):
    print(f"Processing {img_path} with rembg (slicing) ...")
    try:
        img = Image.open(img_path).convert("RGBA")
        
        # Resize first to save massive amounts of RAM and time
        target_height = 1200 # 400px per row
        aspect_ratio = img.width / img.height
        target_width = int(target_height * aspect_ratio)
        img = img.resize((target_width, target_height), Image.Resampling.LANCZOS)
        
        frame_w = img.width // COLS
        frame_h = img.height // ROWS
        
        # Create a new blank transparent image to hold the processed frames
        out_img = Image.new("RGBA", (img.width, img.height), (0, 0, 0, 0))
        
        for r in range(ROWS):
            for c in range(COLS):
                # Crop the individual frame
                box = (c * frame_w, r * frame_h, (c + 1) * frame_w, (r + 1) * frame_h)
                frame = img.crop(box)
                
                # Convert frame to bytes
                byte_arr = io.BytesIO()
                frame.save(byte_arr, format='PNG')
                input_data = byte_arr.getvalue()
                
                # Use rembg on the single frame
                # rembg is very good at single subjects
                output_data = rembg.remove(input_data)
                
                # Load back processed frame
                processed_frame = Image.open(io.BytesIO(output_data)).convert("RGBA")
                
                # Paste it back into the new spritesheet
                out_img.paste(processed_frame, box)
                
        out_img.save(out_path, "PNG")
        print(f"Saved {out_path}")
    except Exception as e:
        print(f"Failed to process {img_path}: {e}")

def main():
    files = glob.glob(os.path.join(SPRITE_DIR, "*_spritesheet.png"))
    for f in files:
        if f.endswith("_transparent.png"):
            continue
        out_path = f.replace(".png", "_transparent.png")
        process_spritesheet(f, out_path)
        
if __name__ == "__main__":
    main()
