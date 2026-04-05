import sys

path = r"C:\tmp\ik_test\source\src\llama-delta-net.cpp"
with open(path, "r", encoding="utf-8") as f:
    content = f.read()

old = (
    "    v = ggml_permute(ctx0, v, 0, 2, 1, 3);\n"
    "    g = ggml_permute(ctx0, g, 2, 0, 3, 1);\n"
    "    beta = ggml_permute(ctx0, beta, 2, 0, 1, 3);\n"
)
new = (
    "    v = ggml_permute(ctx0, v, 0, 2, 1, 3);\n"
    "    g = ggml_permute(ctx0, g, 2, 0, 3, 1);\n"
    "    g = ggml_cont(ctx0, g);\n"
    "    beta = ggml_permute(ctx0, beta, 2, 0, 1, 3);\n"
    "    beta = ggml_cont(ctx0, beta);\n"
)

if old not in content:
    print("PATCH FAILED: string not found")
    sys.exit(1)

patched = content.replace(old, new, 1)
with open(path, "w", encoding="utf-8") as f:
    f.write(patched)
print("PATCH APPLIED")
