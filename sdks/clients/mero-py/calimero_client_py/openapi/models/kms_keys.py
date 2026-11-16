from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.kms_evidence import KmsEvidence


T = TypeVar("T", bound="KmsKeys")


@_attrs_define
class KmsKeys:
    """What a node verified of the mero-kms replicas it took its keys from: the storage key `merod` fetched, and the data
    disk's key the boot script fetched; each absent without a record.

        Attributes:
            storage (Union[Unset, KmsEvidence]): The KMS quote a node accepted a key under: the KMS host, the mero-kms
                release (and image profile) whose signed policy it matched, the quote's registers (hex), its TCB status, SHA-256
                and when it was verified (Unix seconds); `mock` (absent: false) in development builds only.
            disk (Union[Unset, KmsEvidence]): The KMS quote a node accepted a key under: the KMS host, the mero-kms release
                (and image profile) whose signed policy it matched, the quote's registers (hex), its TCB status, SHA-256 and
                when it was verified (Unix seconds); `mock` (absent: false) in development builds only.
    """

    storage: Union[Unset, "KmsEvidence"] = UNSET
    disk: Union[Unset, "KmsEvidence"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.kms_evidence import KmsEvidence

        storage: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.storage, Unset):
            storage = self.storage.to_dict()

        disk: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.disk, Unset):
            disk = self.disk.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if storage is not UNSET:
            field_dict["storage"] = storage
        if disk is not UNSET:
            field_dict["disk"] = disk

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.kms_evidence import KmsEvidence

        d = dict(src_dict)
        _storage = d.pop("storage", UNSET)
        storage: Union[Unset, KmsEvidence]
        if isinstance(_storage, Unset):
            storage = UNSET
        else:
            storage = KmsEvidence.from_dict(_storage)

        _disk = d.pop("disk", UNSET)
        disk: Union[Unset, KmsEvidence]
        if isinstance(_disk, Unset):
            disk = UNSET
        else:
            disk = KmsEvidence.from_dict(_disk)

        kms_keys = cls(
            storage=storage,
            disk=disk,
        )

        kms_keys.additional_properties = d
        return kms_keys

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
