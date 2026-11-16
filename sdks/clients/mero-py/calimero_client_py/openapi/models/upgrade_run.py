from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.upgrade_run_failed import UpgradeRunFailed


T = TypeVar("T", bound="UpgradeRun")


@_attrs_define
class UpgradeRun:
    """An organization-wide upgrade run: the apps it targets, those done, and why each failed one failed.

    Attributes:
        run_id (str):
        package (str):
        version (str):
        cascade (bool):
        by (str):
        started_at (int):
        apps (list[str]):
        done (list[str]):
        failed (UpgradeRunFailed):
        finished (bool):
        team (Union[Unset, str]):
    """

    run_id: str
    package: str
    version: str
    cascade: bool
    by: str
    started_at: int
    apps: list[str]
    done: list[str]
    failed: "UpgradeRunFailed"
    finished: bool
    team: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.upgrade_run_failed import UpgradeRunFailed

        run_id = self.run_id

        package = self.package

        version = self.version

        cascade = self.cascade

        by = self.by

        started_at = self.started_at

        apps = self.apps

        done = self.done

        failed = self.failed.to_dict()

        finished = self.finished

        team = self.team

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "runId": run_id,
                "package": package,
                "version": version,
                "cascade": cascade,
                "by": by,
                "startedAt": started_at,
                "apps": apps,
                "done": done,
                "failed": failed,
                "finished": finished,
            }
        )
        if team is not UNSET:
            field_dict["team"] = team

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.upgrade_run_failed import UpgradeRunFailed

        d = dict(src_dict)
        run_id = d.pop("runId")

        package = d.pop("package")

        version = d.pop("version")

        cascade = d.pop("cascade")

        by = d.pop("by")

        started_at = d.pop("startedAt")

        apps = cast(list[str], d.pop("apps"))

        done = cast(list[str], d.pop("done"))

        failed = UpgradeRunFailed.from_dict(d.pop("failed"))

        finished = d.pop("finished")

        team = d.pop("team", UNSET)

        upgrade_run = cls(
            run_id=run_id,
            package=package,
            version=version,
            cascade=cascade,
            by=by,
            started_at=started_at,
            apps=apps,
            done=done,
            failed=failed,
            finished=finished,
            team=team,
        )

        upgrade_run.additional_properties = d
        return upgrade_run

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
