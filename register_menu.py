import os
import sys
import winreg

def add_context_menu():
    try:
        # Get absolute paths
        python_exe = sys.executable
        if python_exe.lower().endswith("python.exe"):
            pythonw_exe = python_exe[:-10] + "pythonw.exe"
            if os.path.exists(pythonw_exe):
                python_exe = pythonw_exe
                
        script_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "main.py")
        
        if not os.path.exists(script_path):
            print(f"Error: {script_path} not found.")
            return

        key_path = r"Software\Classes\*\shell\SummonMonster"
        
        # Create key
        key = winreg.CreateKey(winreg.HKEY_CURRENT_USER, key_path)
        winreg.SetValue(key, "", winreg.REG_SZ, "召唤大将怪兽摧毁")
        # Add an icon if possible (using shell32.dll trash icon)
        winreg.SetValueEx(key, "Icon", 0, winreg.REG_SZ, "shell32.dll,32")
        winreg.CloseKey(key)
        
        # Create command subkey
        command_key_path = key_path + r"\command"
        command_key = winreg.CreateKey(winreg.HKEY_CURRENT_USER, command_key_path)
        
        # Command string: python.exe "C:\...\main.py" "%1"
        command_str = f'"{python_exe}" "{script_path}" "%1"'
        winreg.SetValue(command_key, "", winreg.REG_SZ, command_str)
        winreg.CloseKey(command_key)
        
        print("Successfully added context menu entry!")
    except Exception as e:
        print(f"Error adding context menu: {e}")

if __name__ == "__main__":
    add_context_menu()
