#include "context_share.h"

#include <windows.h>

#include <algorithm>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <system_error>

namespace {

// Guards the queue file and the collector mutex below. "Local\" scopes both to
// the current session, matching the per-user registry entry.
constexpr wchar_t kQueueMutexName[] = L"Local\\AkioLocalSend.ContextShare.Queue";

// Exists while a collector is gathering paths. It is only created, checked and
// closed while holding the queue mutex, so a process either queues its paths
// before the collector reads them, or finds no collector and becomes one.
constexpr wchar_t kCollectorMutexName[] = L"Local\\AkioLocalSend.ContextShare.Collector";

// Explorer starts all processes within a few milliseconds, so the collector
// stops waiting once the queue has not grown for this long.
constexpr DWORD kQuietPeriodMs = 300;
constexpr DWORD kMaxCollectMs = 3000;
constexpr DWORD kLockTimeoutMs = 5000;

std::filesystem::path QueueFilePath() {
  wchar_t temp[MAX_PATH + 1];
  DWORD length = ::GetTempPathW(MAX_PATH + 1, temp);
  return std::filesystem::path(std::wstring(temp, length)) / L"AkioLocalSend_context_share.txt";
}

class QueueLock {
 public:
  explicit QueueLock(HANDLE mutex) : mutex_(mutex) {
    DWORD result = ::WaitForSingleObject(mutex_, kLockTimeoutMs);
    // An abandoned mutex (its owner crashed) is still acquired.
    acquired_ = result == WAIT_OBJECT_0 || result == WAIT_ABANDONED;
  }
  ~QueueLock() {
    if (acquired_) {
      ::ReleaseMutex(mutex_);
    }
  }
  QueueLock(const QueueLock&) = delete;
  QueueLock& operator=(const QueueLock&) = delete;

  bool acquired() const { return acquired_; }

 private:
  HANDLE mutex_;
  bool acquired_;
};

void AppendToQueue(const std::filesystem::path& file, const std::vector<std::string>& paths) {
  std::ofstream out(file, std::ios::binary | std::ios::app);
  for (const auto& path : paths) {
    // Windows paths never contain a line break, so one path per line is safe.
    out << path << '\n';
  }
}

std::vector<std::string> TakeQueue(const std::filesystem::path& file) {
  std::vector<std::string> paths;
  {
    std::ifstream in(file, std::ios::binary);
    std::string line;
    while (std::getline(in, line)) {
      if (!line.empty() && line.back() == '\r') {
        line.pop_back();
      }
      if (!line.empty() && std::find(paths.begin(), paths.end(), line) == paths.end()) {
        paths.push_back(line);
      }
    }
  }
  std::error_code ignored;
  std::filesystem::remove(file, ignored);
  return paths;
}

uintmax_t QueueSize(const std::filesystem::path& file) {
  std::error_code error;
  uintmax_t size = std::filesystem::file_size(file, error);
  return error ? 0 : size;
}

}  // namespace

bool CollectContextSharePaths(std::vector<std::string>& arguments) {
  // Utf8FromUtf16 keeps the terminating null character inside each string, so
  // compare (and later store) the C strings only.
  auto flag = std::find_if(arguments.begin(), arguments.end(),
                           [](const std::string& argument) { return std::strcmp(argument.c_str(), kContextShareFlag) == 0; });
  if (flag == arguments.end()) {
    return true;
  }
  arguments.erase(flag);
  for (auto& argument : arguments) {
    argument = std::string(argument.c_str());
  }

  HANDLE queue_mutex = ::CreateMutexW(nullptr, FALSE, kQueueMutexName);
  if (queue_mutex == nullptr) {
    // Cannot coordinate; share this process' own paths.
    return true;
  }

  const std::filesystem::path queue_file = QueueFilePath();
  HANDLE collector = nullptr;
  {
    QueueLock lock(queue_mutex);
    if (!lock.acquired()) {
      ::CloseHandle(queue_mutex);
      return true;
    }
    AppendToQueue(queue_file, arguments);
    ::SetLastError(ERROR_SUCCESS);
    collector = ::CreateMutexW(nullptr, FALSE, kCollectorMutexName);
    if (collector != nullptr && ::GetLastError() == ERROR_ALREADY_EXISTS) {
      // Another process collects; it will pick up the paths queued above.
      ::CloseHandle(collector);
      ::CloseHandle(queue_mutex);
      return false;
    }
  }

  // This process is the collector. Wait until the other processes are done.
  uintmax_t size = QueueSize(queue_file);
  for (DWORD waited = 0; waited < kMaxCollectMs; waited += kQuietPeriodMs) {
    ::Sleep(kQuietPeriodMs);
    uintmax_t new_size = QueueSize(queue_file);
    if (new_size == size) {
      break;
    }
    size = new_size;
  }

  std::vector<std::string> paths;
  {
    QueueLock lock(queue_mutex);
    paths = TakeQueue(queue_file);
    // Close inside the lock: from now on, a new process becomes a collector
    // itself instead of queueing paths that nobody would read.
    if (collector != nullptr) {
      ::CloseHandle(collector);
    }
  }
  ::CloseHandle(queue_mutex);

  if (!paths.empty()) {
    arguments = std::move(paths);
  }
  return true;
}
