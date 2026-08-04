import uiautomation as auto
import sys

def test_point(x, y):
    control = auto.ControlFromPoint(x, y)
    print(f"Control under {x}, {y}: {control.Name} ({control.ClassName})")
    
if __name__ == "__main__":
    if len(sys.argv) > 2:
        test_point(int(sys.argv[1]), int(sys.argv[2]))
