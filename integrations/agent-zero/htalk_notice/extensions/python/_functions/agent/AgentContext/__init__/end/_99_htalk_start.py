from helpers.extension import Extension
from usr.plugins.htalk_notice.helpers.receiver import reconcile


class HtalkStart(Extension):
    def execute(self, **kwargs):
        reconcile()
