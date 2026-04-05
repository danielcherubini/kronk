@echo off
call "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat" x64
if errorlevel 1 exit /b 1
cmake --build C:\tmp\ik_test\cmake --config Release --target llama-server -- -j8
if errorlevel 1 exit /b 1
copy /Y C:\tmp\ik_test\cmake\bin\llama-server.exe C:\Users\dan\AppData\Roaming\kronk\backends\ik_llama\llama-server.exe
copy /Y C:\tmp\ik_test\cmake\bin\llama.dll C:\Users\dan\AppData\Roaming\kronk\backends\ik_llama\llama.dll
copy /Y C:\tmp\ik_test\cmake\bin\ggml.dll C:\Users\dan\AppData\Roaming\kronk\backends\ik_llama\ggml.dll
copy /Y C:\tmp\ik_test\cmake\bin\mtmd.dll C:\Users\dan\AppData\Roaming\kronk\backends\ik_llama\mtmd.dll
echo Done.
