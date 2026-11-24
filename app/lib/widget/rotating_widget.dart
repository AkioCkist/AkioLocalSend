import 'package:flutter/material.dart';
import 'package:localsend_isolates/util/sleep.dart';

class RotatingWidget extends StatefulWidget {
  final Duration duration;
  final bool spinning;
  final bool reverse;
  final Widget child;

  const RotatingWidget({
    required this.duration,
    this.spinning = true,
    this.reverse = false,
    required this.child,
    super.key,
  });

  @override
  State<RotatingWidget> createState() => RotatingWidgetState();
}

class RotatingWidgetState extends State<RotatingWidget> {
  static const _fps = 30;
  static const _tickDuration = 1000 ~/ _fps; // in milliseconds
  static const _maxRadians = 6.28; // 360 degrees in radians
  double _angle = 0; // in radians
  double _anglePerTick = 0;
  bool _looping = false;

  @override
  void initState() {
    super.initState();
    _updateAnglePerTick();
    WidgetsBinding.instance.addPostFrameCallback((_) => _loop());
  }

  @override
  void didUpdateWidget(RotatingWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.duration != widget.duration || oldWidget.reverse != widget.reverse) {
      _updateAnglePerTick();
    }
    if (widget.spinning && !oldWidget.spinning) {
      _loop();
    }
  }

  void _updateAnglePerTick() {
    _anglePerTick = _maxRadians / (widget.duration.inMilliseconds / _tickDuration);
    if (widget.reverse) {
      _anglePerTick = -_anglePerTick;
    }
  }

  /// This loop has a much greater performance than using [AnimationController].
  ///
  /// It only runs while [RotatingWidget.spinning] is true; [didUpdateWidget]
  /// starts it again. A stopped widget (e.g. while the window is minimized or
  /// another tab is shown) must not wake the UI isolate 30 times per second.
  void _loop() async {
    if (_looping) {
      return;
    }
    _looping = true;
    try {
      while (mounted && widget.spinning) {
        await sleepAsync(_tickDuration);
        if (!mounted || !widget.spinning) {
          return;
        }
        setState(() {
          _angle = (_angle + _anglePerTick) % _maxRadians;
        });
      }
    } finally {
      _looping = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    return Transform.rotate(
      angle: _angle,
      // Rotating only changes the transform: the child keeps its painted layer
      // instead of being repainted (e.g. the color filtered logo) every tick.
      child: RepaintBoundary(child: widget.child),
    );
  }
}
