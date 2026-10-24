# Bộ mẫu Python nạp thư viện native: ctypes, numpy.ctypeslib, cffi.
import ctypes
import ctypes.util
import os
from ctypes import CDLL, cdll
import numpy as np
from cffi import FFI

here = os.path.dirname(__file__)
ffi = FFI()

a = ctypes.CDLL(os.path.join(here, "build", "libfast.so"))
b = CDLL("libfast.so.1")
c = cdll.LoadLibrary("./libgoext.dylib")
d = ctypes.cdll.helper
e = np.ctypeslib.load_library("librs_ext", here)
f = ffi.dlopen("fast.dll")
g = ctypes.CDLL(ctypes.util.find_library("m"))
h = ctypes.CDLL(None)
i = ctypes.CDLL(so_path, mode=ctypes.RTLD_GLOBAL)
j = ctypes.CDLL(f"{here}/libfast.so")
k = ctypes.CDLL(f"{here}/lib{name}.so")
s = "ctypes.CDLL('libghost.so')"
# ctypes.CDLL("libcomment.so")
l = ctypes.windll.kernel32
m = ctypes.PyDLL(
    "libfast.so"
)
n = ctypes.CDLL(os.environ["FAST_LIB"])
o = ctypes.CDLL(os.path.join(os.environ["ROOT"], name))
p = ctypes.CDLL(os.path.join(here, "build", libname))
q = ctypes.CDLL("fast", mode=ctypes.RTLD_GLOBAL)
r = np.ctypeslib.load_library(name, "libdir")
