#!/usr/bin/env ruby
# The static CLI archive includes a native helper, without GUI libraries.
# Usage: cli-archive.rb CWW CWW_NOTIFY VERSION MUSL_TARGET OUTPUT_DIRECTORY
require 'fileutils'
require 'tmpdir'

abort 'Expected CWW CWW_NOTIFY VERSION MUSL_TARGET OUTPUT_DIRECTORY [ENGINE_DIRECTORY]' unless (5..6).cover?(ARGV.length)
cli, helper, version, target, output, engine = ARGV
abort 'Invalid version' unless version.match?(/\A[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?\z/)
abort 'Unsupported static target' unless %w[x86_64-unknown-linux-musl aarch64-unknown-linux-musl].include?(target)
[cli, helper].each { |path| abort 'Missing executable' unless File.file?(path) && File.executable?(path) }
root = File.expand_path('../..', __dir__)
if target == 'x86_64-unknown-linux-musl'
  abort 'Missing staged browser engine' unless engine
  abort 'Invalid browser engine' unless system('python3', File.join(root, 'browser/engine/scripts/package.py'), 'validate', engine)
else
  abort 'ARM64 musl retains the legacy browser; do not package a different runtime' if engine
end
output = File.expand_path(output)
FileUtils.mkdir_p(output)
name = "cww-v#{version}-#{target}"
Dir.mktmpdir('.cli-archive.', output) do |scratch|
  package = File.join(scratch, name)
  FileUtils.mkdir_p(File.join(package, 'packaging/systemd'))
  FileUtils.cp_r(engine, File.join(package, 'browser-engine')) if engine
  { cli => 'cww', helper => 'cww-notify' }.each do |source, destination|
    FileUtils.install(source, File.join(package, destination), mode: 0o755)
  end
  %w[README.md PROTOCOL.md CONTROL.md SECURITY.md LICENSE-MIT LICENSE-APACHE].each do |doc|
    FileUtils.install(File.join(root, doc), File.join(package, doc), mode: 0o644)
  end
  FileUtils.install(File.join(root, 'packaging/systemd/cww.service'), File.join(package, 'packaging/systemd/cww.service'), mode: 0o644)
  # A CLI-only desktop installation still needs the notification's identity
  # and icon. The entry is hidden from launchers and opens the terminal UI.
  FileUtils.install(File.join(root, 'packaging/linux/cww-notify.desktop'), File.join(package, 'cww-app.desktop'), mode: 0o644)
  FileUtils.install(File.join(root, 'app/assets/mark.svg'), File.join(package, 'cww-app.svg'), mode: 0o644)
  archive = File.join(output, "#{name}.tar.gz")
  abort 'Archive creation failed' unless system('tar', '-C', scratch, '-czf', archive, name)
end
