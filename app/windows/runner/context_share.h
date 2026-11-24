#ifndef RUNNER_CONTEXT_SHARE_H_
#define RUNNER_CONTEXT_SHARE_H_

#include <string>
#include <vector>

// Passed by the Explorer context menu entry in front of the selected path.
// Must match `_contextShareFlag` in app/lib/util/native/context_menu_helper.dart.
inline constexpr char kContextShareFlag[] = "--context-share";

// Explorer starts the context menu command once per selected item, so
// selecting 20 files starts 20 processes at nearly the same time. This merges
// them before the Flutter engine starts: every process appends its paths to a
// shared queue, and only the first one (the collector) keeps running and takes
// all queued paths.
//
// Does nothing unless |arguments| contains kContextShareFlag. Returns false
// when this process must exit because another process collects its paths.
// Otherwise returns true and replaces |arguments| with the paths to share.
bool CollectContextSharePaths(std::vector<std::string>& arguments);

#endif  // RUNNER_CONTEXT_SHARE_H_
