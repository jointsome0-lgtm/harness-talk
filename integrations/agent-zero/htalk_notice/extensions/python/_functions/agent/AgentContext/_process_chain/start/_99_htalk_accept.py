from helpers.extension import Extension
from usr.plugins.htalk_notice.helpers.receiver import accept_wake


class HtalkAccept(Extension):
    def execute(self, data, **kwargs):
        accept_wake(data)
