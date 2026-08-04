import uiautomation as auto

def test():
    auto.SetGlobalSearchTimeout(3.0)
    # Search for anything on the desktop
    desktop = auto.GetRootControl()
    
    # Let's just find the first ListItemControl and print its name
    for item, depth in auto.WalkTree(desktop, getChildren=lambda c: c.GetChildren(), maxDepth=5):
        if isinstance(item, auto.ListItemControl):
            print(f"Found item: {item.Name}, Rect: {item.BoundingRectangle}")
            break

if __name__ == "__main__":
    test()
