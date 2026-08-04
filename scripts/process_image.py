import sys
from PIL import Image

def process_image(input_path, output_path):
    try:
        img = Image.open(input_path).convert("RGBA")
    except Exception as e:
        print(f"Error opening image: {e}")
        return

    width, height = img.size
    
    # We assume the image has 3 views: front, side, back. 
    # The middle view is the side view.
    crop_rect = (width // 3, 0, 2 * (width // 3), height)
    img_cropped = img.crop(crop_rect)

    # Make white background transparent
    datas = img_cropped.getdata()
    new_data = []
    
    # Threshold for considering a pixel "white" (to catch anti-aliasing near edges, though a floodfill is better, 
    # simple thresholding works for clear backgrounds)
    threshold = 240
    for item in datas:
        # item is (R, G, B, A)
        if item[0] > threshold and item[1] > threshold and item[2] > threshold:
            # Change all white (also shades of whites)
            # to transparent
            new_data.append((255, 255, 255, 0))
        else:
            new_data.append(item)
            
    img_cropped.putdata(new_data)
    
    # Crop to bounding box to remove excess transparent space
    bbox = img_cropped.getbbox()
    if bbox:
        img_cropped = img_cropped.crop(bbox)
        
    img_cropped.save(output_path, "PNG")
    print(f"Successfully saved processed sprite to {output_path}")

if __name__ == '__main__':
    process_image("raw_image.png", "monster_sprite.png")
