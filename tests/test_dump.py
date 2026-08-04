import uiautomation as auto
import sys

def find_on_desktop():
    # Try finding the item on the desktop explicitly
    desktop_pane = auto.PaneControl(searchDepth=1, ClassName='Progman')
    if not desktop_pane.Exists(0, 0):
        # Try WorkerW
        for workerw in auto.GetRootControl().GetChildren():
            if workerw.ClassName == 'WorkerW':
                list_control = workerw.ListControl()
                if list_control.Exists(0,0):
                    desktop_pane = workerw
                    break
                    
    if desktop_pane and desktop_pane.Exists(0,0):
        print(f"Found desktop pane: {desktop_pane.ClassName}")
        for item, depth in auto.WalkTree(desktop_pane, getChildren=lambda c: c.GetChildren(), returnServer=False, maxDepth=4):
            if isinstance(item, auto.ListItemControl):
                print(f"Item: {item.Name}")
                
if __name__ == "__main__":
    find_on_desktop()
