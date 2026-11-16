from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="KmsEvidence")


@_attrs_define
class KmsEvidence:
    """The KMS quote a node accepted a key under: the KMS host, the mero-kms release (and image profile) whose signed
    policy it matched, the quote's registers (hex), its TCB status, SHA-256 and when it was verified (Unix seconds);
    `mock` (absent: false) in development builds only.

        Attributes:
            kms_host (str):
            release (str):
            mrtd (str):
            rtmr0 (str):
            rtmr1 (str):
            rtmr2 (str):
            rtmr3 (str):
            quote_sha_256 (str):
            verified_at (int):
            profile (Union[Unset, str]):
            tcb_status (Union[Unset, str]):
            mock (Union[Unset, bool]):
    """

    kms_host: str
    release: str
    mrtd: str
    rtmr0: str
    rtmr1: str
    rtmr2: str
    rtmr3: str
    quote_sha_256: str
    verified_at: int
    profile: Union[Unset, str] = UNSET
    tcb_status: Union[Unset, str] = UNSET
    mock: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kms_host = self.kms_host

        release = self.release

        mrtd = self.mrtd

        rtmr0 = self.rtmr0

        rtmr1 = self.rtmr1

        rtmr2 = self.rtmr2

        rtmr3 = self.rtmr3

        quote_sha_256 = self.quote_sha_256

        verified_at = self.verified_at

        profile = self.profile

        tcb_status = self.tcb_status

        mock = self.mock

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kmsHost": kms_host,
                "release": release,
                "mrtd": mrtd,
                "rtmr0": rtmr0,
                "rtmr1": rtmr1,
                "rtmr2": rtmr2,
                "rtmr3": rtmr3,
                "quoteSha256": quote_sha_256,
                "verifiedAt": verified_at,
            }
        )
        if profile is not UNSET:
            field_dict["profile"] = profile
        if tcb_status is not UNSET:
            field_dict["tcbStatus"] = tcb_status
        if mock is not UNSET:
            field_dict["mock"] = mock

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kms_host = d.pop("kmsHost")

        release = d.pop("release")

        mrtd = d.pop("mrtd")

        rtmr0 = d.pop("rtmr0")

        rtmr1 = d.pop("rtmr1")

        rtmr2 = d.pop("rtmr2")

        rtmr3 = d.pop("rtmr3")

        quote_sha_256 = d.pop("quoteSha256")

        verified_at = d.pop("verifiedAt")

        profile = d.pop("profile", UNSET)

        tcb_status = d.pop("tcbStatus", UNSET)

        mock = d.pop("mock", UNSET)

        kms_evidence = cls(
            kms_host=kms_host,
            release=release,
            mrtd=mrtd,
            rtmr0=rtmr0,
            rtmr1=rtmr1,
            rtmr2=rtmr2,
            rtmr3=rtmr3,
            quote_sha_256=quote_sha_256,
            verified_at=verified_at,
            profile=profile,
            tcb_status=tcb_status,
            mock=mock,
        )

        kms_evidence.additional_properties = d
        return kms_evidence

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
