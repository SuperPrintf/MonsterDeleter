import os
import glob
import numpy as np
from PIL import Image

SPRITE_DIR = r"E:\19-AI项目\删除文件助手"
TOLERANCE = 25 # Tolerance for color distance

def remove_background(img_path, out_path):
    print(f"Processing {img_path} ...")
    img = Image.open(img_path).convert("RGBA")
    data = np.array(img)
    
    # Get top-left pixel as reference background color
    ref_color = data[0, 0, :3].astype(np.int32)
    
    # Calculate Euclidean distance of RGB channels
    rgb = data[:, :, :3].astype(np.int32)
    diff = rgb - ref_color
    dist = np.sqrt(np.sum(diff**2, axis=2))
    
    # Create mask where distance < TOLERANCE
    mask = dist < TOLERANCE
    
    # Set alpha to 0 for matching pixels
    data[mask, 3] = 0
    
    # Save processed image
    processed_img = Image.fromarray(data)
    processed_img.save(out_path, "PNG")
    print(f"Saved {out_path}")

def main():
    files = glob.glob(os.path.join(SPRITE_DIR, "*_spritesheet.png"))
    for f in files:
        if f.endswith("_transparent.png"):
            continue
        out_path = f.replace(".png", "_transparent.png")
        remove_background(f, out_path)
        
if __name__ == "__main__":
    main()
