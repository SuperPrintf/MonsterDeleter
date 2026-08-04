import sys
import os

try:
    import uiautomation as auto
except ImportError:
    print("uiautomation not installed")
    sys.exit(1)

def find_file_pos(filepath):
    filename = os.path.basename(filepath)
    filename_no_ext = os.path.splitext(filename)[0]
    
    print(f"Searching for: {filename} or {filename_no_ext}")
    
    # Since desktop/explorer structures vary (Win10 vs Win11),
    # let's try a direct search for the ListItemControl globally, but with a timeout
    auto.SetGlobalSearchTimeout(1.0)
    
    item = auto.ListItemControl(Name=filename)
    if not item.Exists(0, 0):
        item = auto.ListItemControl(Name=filename_no_ext)
        
    if item.Exists(0, 0):
        rect = item.BoundingRectangle
        print(f"Found globally! Rect: {rect.left}, {rect.top}")
        return rect
        
    print("Not found.")
    return None

if __name__ == "__main__":
    if len(sys.argv) > 1:
        find_file_pos(sys.argv[1])
