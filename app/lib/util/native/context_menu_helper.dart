import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:logging/logging.dart';

final _logger = Logger('ContextMenuHelper');

/// Adds the "Share with Akio Local Send" entry to the Explorer context menu of
/// files and folders, plus the "Send to" shortcut.
Future<bool> enableContextMenu() async {
  if (defaultTargetPlatform != TargetPlatform.windows) {
    return false;
  }

  final verbAdded = await _addShellVerb();

  try {
    final String script =
        '''
\$TargetPath = "${Platform.resolvedExecutable}"
\$ShortcutFile = "${_getWindowsFilePath(_windowsFileName)}"
\$WScriptShell = New-Object -ComObject WScript.Shell
\$Shortcut = \$WScriptShell.CreateShortcut(\$ShortcutFile)
\$Shortcut.TargetPath = \$TargetPath
\$Shortcut.Save()
''';
    final result = await Process.run('powershell', ['-Command', script]);
    if (result.stderr != null && result.stderr!.isNotEmpty) {
      throw Exception('Failed to create shortcut: ${result.stderr}');
    }
    return await File(_getWindowsFilePath(_windowsFileName)).exists() || verbAdded;
  } catch (e) {
    _logger.severe('Failed to enable context menu: $e');
    return verbAdded;
  }
}

Future<bool> disableContextMenu() async {
  try {
    switch (defaultTargetPlatform) {
      case TargetPlatform.windows:
        await _removeShellVerb();
        final file = File(_getWindowsFilePath(_windowsFileName));
        if (await file.exists()) {
          await file.delete();
        }
        return true;
      default:
        return false;
    }
  } catch (e) {
    return false;
  }
}

Future<bool> isContextMenuEnabled() async {
  switch (defaultTargetPlatform) {
    case TargetPlatform.windows:
      return await _isShellVerbRegistered() || await File(_getWindowsFilePath(_windowsFileName)).exists();
    default:
      return false;
  }
}

/// Rewrites the context menu entry when it is enabled, so that it points to
/// this executable even after the app was moved, and so that installations
/// that only had the "Send to" shortcut get the entry too.
Future<void> refreshContextMenu() async {
  if (defaultTargetPlatform != TargetPlatform.windows) {
    return;
  }
  try {
    if (await isContextMenuEnabled()) {
      await _addShellVerb();
    }
  } catch (e) {
    _logger.warning('Failed to refresh context menu: $e');
  }
}

const _windowsFileName = 'LocalSend';

String _getWindowsFilePath(String appName) {
  final appData = Platform.environment['APPDATA'];
  return '$appData/Microsoft/Windows/SendTo/$appName.lnk';
}

const _verbName = 'AkioLocalSend';
const _verbLabel = 'Share with Akio Local Send';

/// Must match `kContextShareFlag` in app/windows/runner/context_share.h, which
/// merges the one process per selected item that Explorer starts.
const _contextShareFlag = '--context-share';

/// `*` covers every file type, `Directory` covers folders. HKCU needs no admin rights.
const _verbKeys = [
  r'HKEY_CURRENT_USER\Software\Classes\*\shell\' + _verbName,
  r'HKEY_CURRENT_USER\Software\Classes\Directory\shell\' + _verbName,
];

Future<bool> _addShellVerb() async {
  final exe = Platform.resolvedExecutable;
  final buffer = StringBuffer('Windows Registry Editor Version 5.00\r\n');
  for (final key in _verbKeys) {
    buffer
      ..write('\r\n[$key]\r\n')
      ..write('@=${_regString(_verbLabel)}\r\n')
      ..write('"Icon"=${_regString('"$exe",0')}\r\n')
      // Shows the entry for any number of selected items (without it, Explorer hides it above 15).
      ..write('"MultiSelectModel"="Player"\r\n')
      ..write('\r\n[$key\\command]\r\n')
      ..write('@=${_regString('"$exe" $_contextShareFlag "%1"')}\r\n');
  }

  final dir = await Directory.systemTemp.createTemp('localsend_context_menu');
  try {
    final file = File('${dir.path}\\context_menu.reg');
    // .reg files are UTF-16 LE with a byte order mark.
    final units = buffer.toString().codeUnits;
    final bytes = Uint8List(2 + units.length * 2)
      ..[0] = 0xFF
      ..[1] = 0xFE;
    for (var i = 0; i < units.length; i++) {
      bytes[2 + i * 2] = units[i] & 0xFF;
      bytes[3 + i * 2] = units[i] >> 8;
    }
    await file.writeAsBytes(bytes);

    final result = await Process.run('reg', ['import', file.path]);
    if (result.exitCode != 0) {
      _logger.severe('Failed to add context menu entry: ${result.stderr}');
      return false;
    }
    return true;
  } catch (e) {
    _logger.severe('Failed to add context menu entry: $e');
    return false;
  } finally {
    try {
      await dir.delete(recursive: true);
    } catch (_) {}
  }
}

Future<void> _removeShellVerb() async {
  for (final key in _verbKeys) {
    // Fails harmlessly when the key does not exist.
    await Process.run('reg', ['delete', key, '/f']);
  }
}

Future<bool> _isShellVerbRegistered() async {
  try {
    final result = await Process.run('reg', ['query', '${_verbKeys.first}\\command', '/ve']);
    return result.exitCode == 0;
  } catch (_) {
    return false;
  }
}

/// Quotes [value] as a .reg file string.
String _regString(String value) => '"${value.replaceAll(r'\', r'\\').replaceAll('"', r'\"')}"';
