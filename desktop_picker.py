"""Run the native file dialog on the main thread of a short-lived process."""

import json
import sys
import tkinter as tk
from tkinter import filedialog


if __name__ == '__main__':
    root = tk.Tk()
    try:
        root.title('VTX file picker')
        root.resizable(False, False)
        root.attributes('-topmost', True)
        tk.Label(root, text='Choose a model in the Windows file dialog.', padx=20, pady=16).pack()
        root.update()
        path = filedialog.askopenfilename(
            parent=root, title='Open a model in VTX',
            filetypes=[('3D models', tuple(sys.argv[1:]))],
        )
        print(json.dumps(path))
    finally:
        root.destroy()
