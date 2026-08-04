import os
import tempfile
from pynput import mouse

def on_click(x, y, button, pressed):
    if button == mouse.Button.right and pressed:
        # Save the coordinates to a temporary file
        temp_dir = tempfile.gettempdir()
        file_path = os.path.join(temp_dir, "last_right_click.txt")
        try:
            with open(file_path, "w") as f:
                f.write(f"{int(x)},{int(y)}")
        except Exception:
            pass

def start_tracking():
    print("Mouse tracker started. Listening for right clicks...")
    with mouse.Listener(on_click=on_click) as listener:
        listener.join()

if __name__ == "__main__":
    start_tracking()
