from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.app_restore_source import AppRestoreSource


T = TypeVar("T", bound="AppRestore")


@_attrs_define
class AppRestore:
    """
    Attributes:
        seed (Union[Unset, int]):
        source (Union[Unset, AppRestoreSource]):
    """

    seed: Union[Unset, int] = UNSET
    source: Union[Unset, "AppRestoreSource"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_restore_source import AppRestoreSource

        seed = self.seed

        source: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.source, Unset):
            source = self.source.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if seed is not UNSET:
            field_dict["seed"] = seed
        if source is not UNSET:
            field_dict["source"] = source

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_restore_source import AppRestoreSource

        d = dict(src_dict)
        seed = d.pop("seed", UNSET)

        _source = d.pop("source", UNSET)
        source: Union[Unset, AppRestoreSource]
        if isinstance(_source, Unset):
            source = UNSET
        else:
            source = AppRestoreSource.from_dict(_source)

        app_restore = cls(
            seed=seed,
            source=source,
        )

        app_restore.additional_properties = d
        return app_restore

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
