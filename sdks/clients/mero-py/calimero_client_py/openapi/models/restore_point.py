from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.package_ref import PackageRef
    from ..models.restore_point_file import RestorePointFile


T = TypeVar("T", bound="RestorePoint")


@_attrs_define
class RestorePoint:
    """An app's one undo (`POST /apps/{app}/undo`): the release and bundle its last upgrade moved it from (`fromRelease`,
    `fromBundle`) and to, when (`at`, the start of the week to undo it) and by whom (`policy:auto-update` for an update
    that applied on its own), the run it was part of (a unit's members undo with their instance), the app group's
    snapshot (`file`: the node that holds it, absent where the upgrade ran nothing on the group) and `undoneAt` while an
    undo's restore is under way.

        Attributes:
            from_bundle (str):
            to_bundle (str):
            at (int):
            by (str):
            from_compute (Union[Unset, str]):
            from_release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-
                service bundle, `compute` (absent: false) marks the service's compute component.
            to_release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-
                service bundle, `compute` (absent: false) marks the service's compute component.
            run (Union[Unset, str]):
            file (Union[Unset, RestorePointFile]):
            undone_at (Union[Unset, int]):
    """

    from_bundle: str
    to_bundle: str
    at: int
    by: str
    from_compute: Union[Unset, str] = UNSET
    from_release: Union[Unset, "PackageRef"] = UNSET
    to_release: Union[Unset, "PackageRef"] = UNSET
    run: Union[Unset, str] = UNSET
    file: Union[Unset, "RestorePointFile"] = UNSET
    undone_at: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.package_ref import PackageRef
        from ..models.restore_point_file import RestorePointFile

        from_bundle = self.from_bundle

        to_bundle = self.to_bundle

        at = self.at

        by = self.by

        from_compute = self.from_compute

        from_release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.from_release, Unset):
            from_release = self.from_release.to_dict()

        to_release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.to_release, Unset):
            to_release = self.to_release.to_dict()

        run = self.run

        file: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.file, Unset):
            file = self.file.to_dict()

        undone_at = self.undone_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "fromBundle": from_bundle,
                "toBundle": to_bundle,
                "at": at,
                "by": by,
            }
        )
        if from_compute is not UNSET:
            field_dict["fromCompute"] = from_compute
        if from_release is not UNSET:
            field_dict["fromRelease"] = from_release
        if to_release is not UNSET:
            field_dict["toRelease"] = to_release
        if run is not UNSET:
            field_dict["run"] = run
        if file is not UNSET:
            field_dict["file"] = file
        if undone_at is not UNSET:
            field_dict["undoneAt"] = undone_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.package_ref import PackageRef
        from ..models.restore_point_file import RestorePointFile

        d = dict(src_dict)
        from_bundle = d.pop("fromBundle")

        to_bundle = d.pop("toBundle")

        at = d.pop("at")

        by = d.pop("by")

        from_compute = d.pop("fromCompute", UNSET)

        _from_release = d.pop("fromRelease", UNSET)
        from_release: Union[Unset, PackageRef]
        if isinstance(_from_release, Unset):
            from_release = UNSET
        else:
            from_release = PackageRef.from_dict(_from_release)

        _to_release = d.pop("toRelease", UNSET)
        to_release: Union[Unset, PackageRef]
        if isinstance(_to_release, Unset):
            to_release = UNSET
        else:
            to_release = PackageRef.from_dict(_to_release)

        run = d.pop("run", UNSET)

        _file = d.pop("file", UNSET)
        file: Union[Unset, RestorePointFile]
        if isinstance(_file, Unset):
            file = UNSET
        else:
            file = RestorePointFile.from_dict(_file)

        undone_at = d.pop("undoneAt", UNSET)

        restore_point = cls(
            from_bundle=from_bundle,
            to_bundle=to_bundle,
            at=at,
            by=by,
            from_compute=from_compute,
            from_release=from_release,
            to_release=to_release,
            run=run,
            file=file,
            undone_at=undone_at,
        )

        restore_point.additional_properties = d
        return restore_point

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
