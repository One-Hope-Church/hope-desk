// Hope Desk v2.1: "Update available" banner state for the staff and Manager apps.
// The Rust side (hope_desk_update::get_common / set_common) owns all update logic;
// this file only polls its status and starts the install. Production updates
// silently as a root service and never shows a banner.
import 'dart:async';
import 'dart:convert';

import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:get/get.dart';

/// One of: none | available | installing | busy | failed | needs-move.
/// Read it inside an Obx so the banner stays reactive.
final RxString hopeDeskUpdateState = 'none'.obs;

Timer? _hopeDeskUpdateTimer;
DateTime? _hopeDeskInstallRequestedAt;

const _kHopeDeskStates = {
  'none',
  'available',
  'installing',
  'busy',
  'failed',
  'needs-move',
};

/// Starts the 5 s status poll. Staff and Manager only; safe to call repeatedly.
void startHopeDeskUpdatePoller() {
  if (_hopeDeskUpdateTimer != null) return;
  if (isHopeDeskProduction()) return; // production updates silently
  _hopeDeskUpdateTimer = Timer.periodic(const Duration(seconds: 5), (_) {
    _hopeDeskPollOnce();
  });
  _hopeDeskPollOnce();
}

Future<void> _hopeDeskPollOnce() async {
  try {
    final raw = await bind.mainGetCommon(key: 'hope-desk-update-status');
    if (raw.isEmpty) return;
    final decoded = jsonDecode(raw);
    if (decoded is! Map) return;
    final state = decoded['state'];
    if (state is! String || !_kHopeDeskStates.contains(state)) return;
    // Right after a click, ignore a stale "available" so the banner doesn't flicker
    // back before the Rust side has moved to "installing".
    final requestedAt = _hopeDeskInstallRequestedAt;
    if (state == 'available' &&
        hopeDeskUpdateState.value == 'installing' &&
        requestedAt != null &&
        DateTime.now().difference(requestedAt) < const Duration(seconds: 6)) {
      return;
    }
    if (hopeDeskUpdateState.value != state) {
      hopeDeskUpdateState.value = state;
    }
  } catch (_) {
    // No update info is never an error the user should see.
  }
}

/// Starts the install. Rust refuses (state "busy") if a remote session is open,
/// and returns to "available" if the macOS admin prompt is cancelled.
Future<void> hopeDeskInstallUpdate() async {
  _hopeDeskInstallRequestedAt = DateTime.now();
  hopeDeskUpdateState.value = 'installing';
  try {
    await bind.mainSetCommon(key: 'hope-desk-update-install', value: '');
  } catch (_) {
    hopeDeskUpdateState.value = 'failed';
    return;
  }
  await _hopeDeskPollOnce();
}

/// Text and button for the banner, or null when no banner should show.
class HopeDeskUpdateBanner {
  final String text;
  final String button; // "" means no button
  final Future<void> Function()? onTap;
  const HopeDeskUpdateBanner(this.text, this.button, this.onTap);
}

/// Call inside an Obx (it reads [hopeDeskUpdateState]).
HopeDeskUpdateBanner? hopeDeskUpdateBanner() {
  final state = hopeDeskUpdateState.value;
  if (isHopeDeskProduction()) return null;
  final appName = isHopeDeskManager() ? 'Hope Desk Manager' : 'Hope Desk';
  switch (state) {
    case 'available':
      return HopeDeskUpdateBanner(
          'A new version of $appName is ready.', 'Update', hopeDeskInstallUpdate);
    case 'busy':
      return HopeDeskUpdateBanner(
          'Finish your remote session first.', 'Update', hopeDeskInstallUpdate);
    case 'installing':
      return HopeDeskUpdateBanner(
          'Updating. $appName will reopen in a moment.', '', null);
    case 'failed':
      return HopeDeskUpdateBanner(
          "The update couldn't be installed. You're still on the current version.",
          'Try again',
          hopeDeskInstallUpdate);
    case 'needs-move':
      return HopeDeskUpdateBanner(
          'Move $appName to the Applications folder to update it.', '', null);
    default:
      return null;
  }
}
