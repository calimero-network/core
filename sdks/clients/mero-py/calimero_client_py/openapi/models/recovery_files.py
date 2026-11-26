from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.recovery_files_files_item import RecoveryFilesFilesItem
    from ..models.recovery_files_holder_type_0 import RecoveryFilesHolderType0
    from ..models.recovery_files_holder_type_1 import RecoveryFilesHolderType1
    from ..models.recovery_files_holder_type_2 import RecoveryFilesHolderType2


T = TypeVar("T", bound="RecoveryFiles")


@_attrs_define
class RecoveryFiles:
    """`GET /ops/recovery-files`: a holder and its recovery files, each with its signed header.

    Attributes:
        holder (Union['RecoveryFilesHolderType0', 'RecoveryFilesHolderType1', 'RecoveryFilesHolderType2']):
        files (list['RecoveryFilesFilesItem']):
    """

    holder: Union[
        "RecoveryFilesHolderType0",
        "RecoveryFilesHolderType1",
        "RecoveryFilesHolderType2",
    ]
    files: list["RecoveryFilesFilesItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.recovery_files_files_item import RecoveryFilesFilesItem
        from ..models.recovery_files_holder_type_0 import RecoveryFilesHolderType0
        from ..models.recovery_files_holder_type_1 import RecoveryFilesHolderType1
        from ..models.recovery_files_holder_type_2 import RecoveryFilesHolderType2

        holder: dict[str, Any]
        if isinstance(self.holder, RecoveryFilesHolderType0):
            holder = self.holder.to_dict()
        elif isinstance(self.holder, RecoveryFilesHolderType1):
            holder = self.holder.to_dict()
        else:
            holder = self.holder.to_dict()

        files = []
        for files_item_data in self.files:
            files_item = files_item_data.to_dict()
            files.append(files_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "holder": holder,
                "files": files,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.recovery_files_files_item import RecoveryFilesFilesItem
        from ..models.recovery_files_holder_type_0 import RecoveryFilesHolderType0
        from ..models.recovery_files_holder_type_1 import RecoveryFilesHolderType1
        from ..models.recovery_files_holder_type_2 import RecoveryFilesHolderType2

        d = dict(src_dict)

        def _parse_holder(
            data: object,
        ) -> Union[
            "RecoveryFilesHolderType0",
            "RecoveryFilesHolderType1",
            "RecoveryFilesHolderType2",
        ]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                holder_type_0 = RecoveryFilesHolderType0.from_dict(data)

                return holder_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                holder_type_1 = RecoveryFilesHolderType1.from_dict(data)

                return holder_type_1
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            holder_type_2 = RecoveryFilesHolderType2.from_dict(data)

            return holder_type_2

        holder = _parse_holder(d.pop("holder"))

        files = []
        _files = d.pop("files")
        for files_item_data in _files:
            files_item = RecoveryFilesFilesItem.from_dict(files_item_data)

            files.append(files_item)

        recovery_files = cls(
            holder=holder,
            files=files,
        )

        recovery_files.additional_properties = d
        return recovery_files

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
