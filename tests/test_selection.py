import sys
import time
import uiautomation as auto

def find_selected_item():
    try:
        # Wait a moment for context menu to disappear or window to stabilize if needed
        # Actually, when invoked from context menu, the menu disappears.
        
        # We can search globally for a control that supports SelectionItemPattern and is selected.
        # But global search can be slow. Let's just find the first selected ListItemControl.
        
        # Set a short timeout
        auto.SetGlobalSearchTimeout(2.0)
        
        desktop = auto.GetRootControl()
        
        # Find all selected items (this might be slow if we search the whole tree, so let's limit depth)
        # Actually, if we just right clicked it, it's on the desktop or active explorer window.
        
        # Try foreground window first
        foreground = auto.GetForegroundControl()
        print(f"Foreground window: {foreground.Name} ({foreground.ClassName})")
        
        selected_items = []
        for item, depth in auto.WalkTree(foreground, getChildren=lambda c: c.GetChildren(), returnServer=False, maxDepth=4):
            if isinstance(item, auto.ListItemControl) or isinstance(item, auto.TreeItemControl):
                try:
                    if item.GetSelectionItemPattern().IsSelected:
                        selected_items.append(item)
                except Exception:
                    pass
                    
        if selected_items:
            rect = selected_items[0].BoundingRectangle
            print(f"Found selected item in foreground! Rect: {rect.left}, {rect.top}")
            return
            
        # If not found in foreground, search desktop specifically
        progman = auto.PaneControl(searchDepth=1, ClassName='Progman')
        if progman.Exists(0, 0):
            for item, depth in auto.WalkTree(progman, getChildren=lambda c: c.GetChildren(), returnServer=False, maxDepth=4):
                if isinstance(item, auto.ListItemControl):
                    try:
                        if item.GetSelectionItemPattern().IsSelected:
                            rect = item.BoundingRectangle
                            print(f"Found selected item on Desktop (Progman)! Rect: {rect.left}, {rect.top}")
                            return
                    except Exception:
                        pass

        # Windows 11 Desktop (WorkerW)
        for workerw in auto.GetRootControl().GetChildren():
            if workerw.ClassName == 'WorkerW':
                for item, depth in auto.WalkTree(workerw, getChildren=lambda c: c.GetChildren(), returnServer=False, maxDepth=4):
                    if isinstance(item, auto.ListItemControl):
                        try:
                            if item.GetSelectionItemPattern().IsSelected:
                                rect = item.BoundingRectangle
                                print(f"Found selected item on Desktop (WorkerW)! Rect: {rect.left}, {rect.top}")
                                return
                        except Exception:
                            pass
                            
        print("No selected item found.")
    except Exception as e:
        print(f"Error: {e}")

if __name__ == "__main__":
    # We will simulate this by selecting a file and then running the script
    find_selected_item()
