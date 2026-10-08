@echo off
rem Run by tests.yml and publish.yml inside a Windows container, next to the executable.
rem A runner has the Visual C++ runtime and a person's machine may not, so a start on the runner
rem does not show that the executable needs none. This must be a Windows without it.
if exist %SystemRoot%\System32\vcruntime140.dll (echo this image has the Visual C++ runtime, so a start in it shows nothing & exit 1)
set H=%~dp0htalk.exe
%H% --version
if not %errorlevel%==0 (echo the executable did not start here, exit %errorlevel% & exit 1)
%H% peer add alice --harness generic --delivery pull || exit 1
%H% peer add bob --harness generic --delivery pull || exit 1
%H% --as alice send bob --message Question || exit 1
%H% --as bob inbox || exit 1
